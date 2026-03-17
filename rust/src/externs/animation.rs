use std::collections::HashMap;

use vo_ext::prelude::*;

use super::render::wasm_debug_log;
use super::util::{read_u32_le, ret_bytes, with_renderer_or_panic};

fn decode_entity_models(data: &[u8]) -> HashMap<u32, u32> {
    assert!(data.len() >= 4, "voplay: animation entity-model map too short");
    let mut pos = 0usize;
    let count = read_u32_le(data, &mut pos) as usize;
    assert!(data.len() == 4 + count * 8, "voplay: animation entity-model map size mismatch");
    let mut map = HashMap::with_capacity(count);
    for _ in 0..count {
        let target_id = read_u32_le(data, &mut pos);
        let model_id = read_u32_le(data, &mut pos);
        map.insert(target_id, model_id);
    }
    map
}

pub(crate) fn serialize_model_animation_info(info: crate::animation::ModelAnimationInfo) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(if info.has_skeleton { 1 } else { 0 });
    buf.extend_from_slice(&(info.joint_count as u32).to_le_bytes());
    buf.extend_from_slice(&(info.clips.len() as u32).to_le_bytes());
    for clip in info.clips {
        buf.extend_from_slice(&clip.duration.to_le_bytes());
        let name = clip.name.into_bytes();
        assert!(name.len() <= u16::MAX as usize, "voplay: animation clip name too long");
        buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
        buf.extend_from_slice(&name);
    }
    buf
}

#[vo_fn("voplay", "animationInit")]
pub fn animation_init(call: &mut ExternCallContext) -> ExternResult {
    let world_id = crate::animation::create_world();
    call.ret_u64(0, world_id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "animationDestroy")]
pub fn animation_destroy(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    crate::animation::destroy_world(world_id);
    ExternResult::Ok
}

#[vo_fn("voplay", "animationPlay")]
pub fn animation_play(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    let clip_index = call.arg_u64(2) as usize;
    let looping = call.arg_bool(3);
    let speed = call.arg_f64(4) as f32;
    crate::animation::with_world(world_id, |world| world.play(target_id, clip_index, looping, speed));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationStop")]
pub fn animation_stop(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    crate::animation::with_world(world_id, |world| world.stop(target_id));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationCrossfade")]
pub fn animation_crossfade(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    let clip_index = call.arg_u64(2) as usize;
    let duration = call.arg_f64(3) as f32;
    crate::animation::with_world(world_id, |world| world.crossfade(target_id, clip_index, duration));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationSetSpeed")]
pub fn animation_set_speed(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    let speed = call.arg_f64(2) as f32;
    crate::animation::with_world(world_id, |world| world.set_speed(target_id, speed));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationRemoveTarget")]
pub fn animation_remove_target(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    crate::animation::with_world(world_id, |world| world.remove(target_id));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationTick")]
pub fn animation_tick(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let dt = call.arg_f64(1) as f32;
    let entity_model_bytes = call.arg_bytes(2);
    wasm_debug_log(&format!(
        "animationTick start world_id={} dt={} entity_model_bytes={}",
        world_id,
        dt,
        entity_model_bytes.len()
    ));
    let entity_models = decode_entity_models(entity_model_bytes);
    wasm_debug_log(&format!(
        "animationTick decoded world_id={} targets={}",
        world_id,
        entity_models.len()
    ));
    with_renderer_or_panic("animationTick", |renderer| {
        renderer.tick_animations(world_id, dt, &entity_models)
    });
    wasm_debug_log(&format!("animationTick done world_id={}", world_id));
    ExternResult::Ok
}

#[vo_fn("voplay", "animationProgress")]
pub fn animation_progress(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let target_id = call.arg_u64(1) as u32;
    let model_id = call.arg_u64(2) as u32;
    let progress = with_renderer_or_panic("animationProgress", |renderer| {
        renderer.animation_progress(world_id, target_id, model_id)
    });
    call.ret_f64(0, progress as f64);
    ExternResult::Ok
}

#[vo_fn("voplay", "animationModelInfo")]
pub fn animation_model_info(call: &mut ExternCallContext) -> ExternResult {
    let model_id = call.arg_u64(0) as u32;
    wasm_debug_log(&format!("animationModelInfo start model_id={}", model_id));
    let info = with_renderer_or_panic("animationModelInfo", |renderer| {
        renderer.get_model_animation_info(model_id)
    })
    .unwrap_or(crate::animation::ModelAnimationInfo {
        has_skeleton: false,
        joint_count: 0,
        clips: vec![],
    });
    wasm_debug_log(&format!(
        "animationModelInfo loaded model_id={} clips={} joints={} has_skeleton={}",
        model_id,
        info.clips.len(),
        info.joint_count,
        info.has_skeleton
    ));
    let data = serialize_model_animation_info(info);
    ret_bytes(call, 0, &data);
    ExternResult::Ok
}
