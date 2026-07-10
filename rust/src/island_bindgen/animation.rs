use super::*;

// ── scene3d animation externs ─────────────────────────────────────────────────

/// animationInit() → uint32
#[wasm_bindgen(js_name = "animationInit")]
pub fn scene3d_animation_init(_input: &[u8]) -> Vec<u8> {
    let world_id = crate::animation::create_world();
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// animationDestroy(worldId)
#[wasm_bindgen(js_name = "animationDestroy")]
pub fn scene3d_animation_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    crate::animation::destroy_world(world_id);
    Vec::new()
}

/// animationPlay(worldId, targetId, clipIndex, looping, speed)
#[wasm_bindgen(js_name = "animationPlay")]
pub fn scene3d_animation_play(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let clip_index = in_value(input, &mut pos) as usize;
    let looping = in_bool(input, &mut pos);
    let speed = in_f64(input, &mut pos) as f32;
    crate::animation::with_world(world_id, |w| w.play(target_id, clip_index, looping, speed));
    Vec::new()
}

/// animationStop(worldId, targetId)
#[wasm_bindgen(js_name = "animationStop")]
pub fn scene3d_animation_stop(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    crate::animation::with_world(world_id, |w| w.stop(target_id));
    Vec::new()
}

/// animationCrossfade(worldId, targetId, clipIndex, duration)
#[wasm_bindgen(js_name = "animationCrossfade")]
pub fn scene3d_animation_crossfade(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let clip_index = in_value(input, &mut pos) as usize;
    let duration = in_f64(input, &mut pos) as f32;
    crate::animation::with_world(world_id, |w| w.crossfade(target_id, clip_index, duration));
    Vec::new()
}

/// animationSetSpeed(worldId, targetId, speed)
#[wasm_bindgen(js_name = "animationSetSpeed")]
pub fn scene3d_animation_set_speed(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let speed = in_f64(input, &mut pos) as f32;
    crate::animation::with_world(world_id, |w| w.set_speed(target_id, speed));
    Vec::new()
}

/// animationRemoveTarget(worldId, targetId)
#[wasm_bindgen(js_name = "animationRemoveTarget")]
pub fn scene3d_animation_remove_target(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    crate::animation::with_world(world_id, |w| w.remove(target_id));
    Vec::new()
}

/// animationTick(worldId, dt, entityModels []byte)
#[wasm_bindgen(js_name = "animationTick")]
pub fn scene3d_animation_tick(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let entity_bytes = in_bytes(input, &mut pos);
    let entity_models = decode_entity_models(entity_bytes);
    crate::externs::util::with_renderer_or_panic("animationTick", |renderer| {
        renderer.tick_animations(world_id, dt, &entity_models)
    });
    Vec::new()
}

fn decode_entity_models(data: &[u8]) -> std::collections::HashMap<u32, u32> {
    assert!(
        data.len() >= 4,
        "voplay: animation entity-model map too short"
    );
    let mut pos = 0usize;
    let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    assert!(
        data.len() == 4 + count * 8,
        "voplay: animation entity-model map size mismatch"
    );
    let mut map = std::collections::HashMap::with_capacity(count);
    for _ in 0..count {
        let target_id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let model_id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;
        map.insert(target_id, model_id);
    }
    map
}

/// animationProgress(worldId, targetId, modelId) → float
#[wasm_bindgen(js_name = "animationProgress")]
pub fn scene3d_animation_progress(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let model_id = in_value(input, &mut pos) as u32;
    let progress = crate::externs::util::with_renderer_or_panic("animationProgress", |r| {
        r.animation_progress(world_id, target_id, model_id)
    });
    let mut out = Vec::new();
    out_value_f64(&mut out, progress as f64);
    out
}

/// animationModelInfo(modelId) → []byte
#[wasm_bindgen(js_name = "animationModelInfo")]
pub fn animation_model_info(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let model_id = in_value(input, &mut pos) as u32;
    let info = crate::externs::util::with_renderer_or_panic("animationModelInfo", |r| {
        r.get_model_animation_info(model_id)
    })
    .unwrap_or(crate::animation::ModelAnimationInfo {
        has_skeleton: false,
        joint_count: 0,
        clips: vec![],
    });
    let data = crate::externs::animation::serialize_model_animation_info(info);
    let mut out = Vec::new();
    out_bytes(&mut out, &data);
    out
}
