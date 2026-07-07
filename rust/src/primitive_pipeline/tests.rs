use super::{
    primitive_render_mode, sort_primitive_batches, PrimitiveBatch, PrimitiveBatchKey,
    PrimitivePipeline, PrimitiveRenderMode, PrimitiveTextureKey,
};
use crate::material::MaterialSamplerKey;
use crate::math3d::{Quat, Vec3};
use crate::model_loader::ModelManager;
use crate::pipeline3d::{MaterialOverride, ModelUniform};
use crate::primitive_scene::{
    PrimitiveDraw, PrimitiveObjectUpdate, PRIMITIVE_FLAG_ATLAS_UV, PRIMITIVE_FLAG_BILLBOARD,
};
use crate::texture::TextureManager;
use bytemuck::Zeroable;

#[test]
fn primitive_pipeline_creates_with_current_shader_layouts() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    else {
        return;
    };
    let adapter_limits = adapter.limits();
    if adapter_limits.max_inter_stage_shader_components < 44 {
        return;
    }
    let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
    limits.max_inter_stage_shader_components = adapter_limits
        .max_inter_stage_shader_components
        .min(wgpu::Limits::default().max_inter_stage_shader_components);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("voplay_primitive_pipeline_test"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .expect("request device");

    let _pipeline = PrimitivePipeline::new(
        &device,
        &queue,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
    );
}

#[test]
fn primitive_pipeline_tracks_resident_chunks() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    else {
        return;
    };
    let adapter_limits = adapter.limits();
    if adapter_limits.max_inter_stage_shader_components < 44 {
        return;
    }
    let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
    limits.max_inter_stage_shader_components = adapter_limits
        .max_inter_stage_shader_components
        .min(wgpu::Limits::default().max_inter_stage_shader_components);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("voplay_primitive_resident_chunk_test"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .expect("request device");
    let mut pipeline = PrimitivePipeline::new(
        &device,
        &queue,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
    );
    let models = ModelManager::new();
    let textures = TextureManager::new(&device);
    let update = PrimitiveObjectUpdate {
        scene_id: 1,
        layer_id: 2,
        object_id: 3,
        model_id: 4,
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
    };
    pipeline.replace_chunk(&device, &queue, 1, 2, 5, &[update], &models, &textures);
    assert_eq!(pipeline.resident_chunks.len(), 1);
    assert_eq!(pipeline.object_chunks.len(), 1);

    pipeline.destroy_instance(&device, &queue, 1, 2, 3, &models, &textures);
    assert_eq!(pipeline.rebuild_queue.len(), 1);
    assert_eq!(
        pipeline.flush_resident_rebuild_queue(&device, &queue, &models),
        1
    );
    assert!(pipeline.resident_chunks.is_empty());
    assert!(pipeline.object_chunks.is_empty());
}

#[test]
fn primitive_pipeline_single_instance_update_uses_partial_upload() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    else {
        return;
    };
    let adapter_limits = adapter.limits();
    if adapter_limits.max_inter_stage_shader_components < 44 {
        return;
    }
    let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
    limits.max_inter_stage_shader_components = adapter_limits
        .max_inter_stage_shader_components
        .min(wgpu::Limits::default().max_inter_stage_shader_components);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("voplay_primitive_partial_upload_test"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .expect("request device");
    let mut pipeline = PrimitivePipeline::new(
        &device,
        &queue,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
    );
    let mut models = ModelManager::new();
    let model_id = models.create_cube(&device, &queue);
    let textures = TextureManager::new(&device);
    let mut update = PrimitiveObjectUpdate {
        scene_id: 1,
        layer_id: 2,
        object_id: 3,
        model_id,
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
    };
    pipeline.replace_chunk(&device, &queue, 1, 2, 5, &[update], &models, &textures);
    assert_eq!(pipeline.resident_chunks.len(), 1);
    assert!(pipeline
        .resident_chunks
        .values()
        .any(|chunk| !chunk.depth_batches.is_empty()));

    update.pos = Vec3 {
        x: 1.0,
        y: 0.25,
        z: -0.5,
    };
    pipeline.upsert_instance(&device, &queue, update, &models, &textures);
    assert_eq!(pipeline.rebuild_queue.len(), 1);
    assert_eq!(
        pipeline.flush_resident_rebuild_queue(&device, &queue, &models),
        0
    );
    assert_eq!(pipeline.last_resident_rebuild_policy.full_rebuild_count, 0);
    assert!(pipeline.last_resident_rebuild_policy.dirty_upload_bytes > 0);
    assert_eq!(
        pipeline.last_resident_rebuild_policy.rebuild_reason,
        "dirty-range-partial-upload"
    );
}

#[test]
fn primitive_pipeline_classifies_render_modes() {
    let mut draw = PrimitiveDraw {
        model_id: 1,
        model_uniform: ModelUniform::zeroed(),
        material: MaterialOverride::default(),
        instance_params: [0.0; 4],
        instance_params2: [0.0, 0.0, 1.0, 1.0],
    };
    assert_eq!(
        primitive_render_mode(&draw, [1.0, 1.0, 1.0, 1.0]),
        PrimitiveRenderMode::Opaque
    );
    assert_eq!(
        primitive_render_mode(&draw, [1.0, 1.0, 1.0, 0.5]),
        PrimitiveRenderMode::Translucent
    );
    draw.instance_params[0] = PRIMITIVE_FLAG_ATLAS_UV as f32;
    assert_eq!(
        primitive_render_mode(&draw, [1.0, 1.0, 1.0, 1.0]),
        PrimitiveRenderMode::Cutout
    );
    draw.instance_params[0] = PRIMITIVE_FLAG_BILLBOARD as f32;
    assert_eq!(
        primitive_render_mode(&draw, [1.0, 1.0, 1.0, 0.5]),
        PrimitiveRenderMode::Cutout
    );
}

#[test]
fn primitive_pipeline_sorts_batches_by_state_then_model() {
    let texture = PrimitiveTextureKey {
        albedo: 0,
        normal: 0,
        metallic_roughness: 0,
        emissive: 0,
        toon_ramp: 0,
        sampler: MaterialSamplerKey::REPEAT_LINEAR,
    };
    let mut batches = vec![
        PrimitiveBatch {
            key: PrimitiveBatchKey {
                model_id: 8,
                mesh_index: 0,
                textures: texture,
                mode: PrimitiveRenderMode::Translucent,
            },
            instances: Vec::new(),
        },
        PrimitiveBatch {
            key: PrimitiveBatchKey {
                model_id: 4,
                mesh_index: 0,
                textures: texture,
                mode: PrimitiveRenderMode::Cutout,
            },
            instances: Vec::new(),
        },
        PrimitiveBatch {
            key: PrimitiveBatchKey {
                model_id: 2,
                mesh_index: 0,
                textures: texture,
                mode: PrimitiveRenderMode::Opaque,
            },
            instances: Vec::new(),
        },
    ];
    sort_primitive_batches(&mut batches);
    assert_eq!(batches[0].key.mode, PrimitiveRenderMode::Opaque);
    assert_eq!(batches[0].key.model_id, 2);
    assert_eq!(batches[1].key.mode, PrimitiveRenderMode::Cutout);
    assert_eq!(batches[1].key.model_id, 4);
    assert_eq!(batches[2].key.mode, PrimitiveRenderMode::Translucent);
    assert_eq!(batches[2].key.model_id, 8);
}
