use super::*;

// ── scene3d animation externs ─────────────────────────────────────────────────

/// animationInit() → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationInit")]
pub fn scene3d_animation_init(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let world_id = crate::animation::create_world();
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// animationDestroy(worldId)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationDestroy")]
pub fn scene3d_animation_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    pos.finish();
    crate::animation::destroy_world(world_id);
    Vec::new()
}

/// animationPlay(worldId, targetId, clipIndex, looping, speed)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationPlay")]
pub fn scene3d_animation_play(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let clip_index =
        crate::externs::animation::decode_animation_clip_index(in_value(input, &mut pos));
    let looping = in_bool(input, &mut pos);
    let speed = in_f64(input, &mut pos) as f32;
    pos.finish();
    crate::animation::with_world(world_id, |w| w.play(target_id, clip_index, looping, speed));
    Vec::new()
}

/// animationStop(worldId, targetId)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationStop")]
pub fn scene3d_animation_stop(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    pos.finish();
    crate::animation::with_world(world_id, |w| w.stop(target_id));
    Vec::new()
}

/// animationCrossfade(worldId, targetId, clipIndex, duration)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationCrossfade")]
pub fn scene3d_animation_crossfade(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let clip_index =
        crate::externs::animation::decode_animation_clip_index(in_value(input, &mut pos));
    let duration = in_f64(input, &mut pos) as f32;
    pos.finish();
    crate::animation::with_world(world_id, |w| w.crossfade(target_id, clip_index, duration));
    Vec::new()
}

/// animationSetSpeed(worldId, targetId, speed)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationSetSpeed")]
pub fn scene3d_animation_set_speed(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let speed = in_f64(input, &mut pos) as f32;
    pos.finish();
    crate::animation::with_world(world_id, |w| w.set_speed(target_id, speed));
    Vec::new()
}

/// animationRemoveTarget(worldId, targetId)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationRemoveTarget")]
pub fn scene3d_animation_remove_target(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    pos.finish();
    crate::animation::with_world(world_id, |w| w.remove(target_id));
    Vec::new()
}

/// animationTick(worldId, dt, entityModels []byte)
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationTick")]
pub fn scene3d_animation_tick(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let entity_bytes = in_bytes(input, &mut pos);
    pos.finish();
    let entity_models = crate::externs::animation::decode_entity_models(entity_bytes);
    crate::externs::util::with_renderer_or_panic("animationTick", |renderer| {
        renderer.tick_animations(world_id, dt, &entity_models)
    });
    Vec::new()
}

/// animationProgress(worldId, targetId, modelId) → float
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationProgress")]
pub fn scene3d_animation_progress(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let target_id = in_value(input, &mut pos) as u32;
    let model_id = in_value(input, &mut pos) as u32;
    pos.finish();
    let progress = crate::externs::util::with_renderer_or_panic("animationProgress", |r| {
        r.animation_progress(world_id, target_id, model_id)
    });
    let mut out = Vec::new();
    out_value_f64(&mut out, progress as f64);
    out
}

/// animationModelInfo(modelId) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "animationModelInfo")]
pub fn animation_model_info(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let model_id = in_value(input, &mut pos) as u32;
    pos.finish();
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

#[cfg(test)]
mod tests {
    use super::{scene3d_animation_crossfade, scene3d_animation_play};

    fn invalid_clip_input() -> Vec<u8> {
        let mut input = Vec::new();
        input.extend_from_slice(&0u64.to_le_bytes());
        input.extend_from_slice(&0u64.to_le_bytes());
        input.extend_from_slice(&(u32::MAX as u64 + 1).to_le_bytes());
        input
    }

    fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
        if let Some(message) = panic.downcast_ref::<String>() {
            return message.clone();
        }
        if let Some(message) = panic.downcast_ref::<&str>() {
            return (*message).to_string();
        }
        "non-string panic".to_string()
    }

    #[test]
    fn animation_wrappers_reject_wide_clip_indices_before_world_access() {
        for wrapper in [
            scene3d_animation_play as fn(&[u8]) -> Vec<u8>,
            scene3d_animation_crossfade,
        ] {
            let input = invalid_clip_input();
            let panic = std::panic::catch_unwind(|| wrapper(&input)).unwrap_err();
            assert!(
                panic_message(panic).contains("animation clip index exceeds u32 range"),
                "wrapper reached another operation before validating clipIndex",
            );
        }
    }
}
