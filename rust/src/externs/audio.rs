//! Audio externs (voplay root package).

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use std::sync::{Mutex, OnceLock};
use crate::audio::AudioEngine;

static AUDIO: OnceLock<Option<Mutex<AudioEngine>>> = OnceLock::new();

fn get_audio() -> Option<&'static Mutex<AudioEngine>> {
    AUDIO.get_or_init(|| {
        match AudioEngine::new() {
            Some(engine) => Some(Mutex::new(engine)),
            None => {
                log::warn!("voplay: no audio device available, audio will be silent");
                None
            }
        }
    }).as_ref()
}

#[vo_fn("voplay", "audioLoad")]
pub fn audio_load(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    match get_audio() {
        Some(audio_mutex) => {
            let mut engine = audio_mutex.lock().unwrap();
            match engine.load_file(&path) {
                Ok(id) => {
                    call.ret_u64(0, id as u64);
                    write_nil_error(call, 1);
                }
                Err(msg) => {
                    call.ret_u64(0, 0);
                    write_error_to(call, 1, &msg);
                }
            }
        }
        None => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, "voplay: no audio device available");
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioLoadBytes")]
pub fn audio_load_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    match get_audio() {
        Some(audio_mutex) => {
            let mut engine = audio_mutex.lock().unwrap();
            match engine.load_bytes(data) {
                Ok(id) => {
                    call.ret_u64(0, id as u64);
                    write_nil_error(call, 1);
                }
                Err(msg) => {
                    call.ret_u64(0, 0);
                    write_error_to(call, 1, &msg);
                }
            }
        }
        None => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, "voplay: no audio device available");
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioFree")]
pub fn audio_free(call: &mut ExternCallContext) -> ExternResult {
    let clip_id = call.arg_u64(0) as u32;
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().free_clip(clip_id);
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioPlaySound")]
pub fn audio_play_sound(call: &mut ExternCallContext) -> ExternResult {
    let clip_id = call.arg_u64(0) as u32;
    let volume = call.arg_f64(1) as f32;
    let pitch = call.arg_f64(2) as f32;
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().play_sound(clip_id, volume, pitch);
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioPlayMusic")]
pub fn audio_play_music(call: &mut ExternCallContext) -> ExternResult {
    let clip_id = call.arg_u64(0) as u32;
    let volume = call.arg_f64(1) as f32;
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().play_music(clip_id, volume);
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioStopMusic")]
pub fn audio_stop_music(_call: &mut ExternCallContext) -> ExternResult {
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().stop_music();
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioPauseMusic")]
pub fn audio_pause_music(_call: &mut ExternCallContext) -> ExternResult {
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().pause_music();
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioResumeMusic")]
pub fn audio_resume_music(_call: &mut ExternCallContext) -> ExternResult {
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().resume_music();
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioSetSFXVolume")]
pub fn audio_set_sfx_volume(call: &mut ExternCallContext) -> ExternResult {
    let vol = call.arg_f64(0) as f32;
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().set_sfx_volume(vol);
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "audioSetMusicVolume")]
pub fn audio_set_music_volume(call: &mut ExternCallContext) -> ExternResult {
    let vol = call.arg_f64(0) as f32;
    if let Some(audio_mutex) = get_audio() {
        audio_mutex.lock().unwrap().set_music_volume(vol);
    }
    ExternResult::Ok
}
