//! Font and model load/free externs (voplay root package).

use std::sync::{Mutex, OnceLock};

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use crate::font_manager::FontManager;

use super::with_renderer;

static HEADLESS_FONT_MANAGER: OnceLock<Mutex<FontManager>> = OnceLock::new();

fn with_headless_font_manager<R>(f: impl FnOnce(&mut FontManager) -> R) -> R {
    let manager = HEADLESS_FONT_MANAGER.get_or_init(|| Mutex::new(FontManager::new()));
    let mut manager = manager.lock().unwrap();
    f(&mut manager)
}

// --- Font externs ---

#[vo_fn("voplay", "loadFont")]
pub fn load_font(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let result = match with_renderer(|r| r.load_font(&path)) {
        Ok(result) => result,
        Err(_) => with_headless_font_manager(|fonts| fonts.load_file(&path)),
    };
    match result {
        Ok(id) => {
            call.ret_u64(0, id as u64);
            write_nil_error(call, 1);
        }
        Err(msg) => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, &msg);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "loadFontBytes")]
pub fn load_font_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    let result = match with_renderer(|r| r.load_font_bytes(data.clone())) {
        Ok(result) => result,
        Err(_) => with_headless_font_manager(|fonts| fonts.load_bytes(data)),
    };
    match result {
        Ok(id) => {
            call.ret_u64(0, id as u64);
            write_nil_error(call, 1);
        }
        Err(msg) => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, &msg);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "freeFont")]
pub fn free_font(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    if with_renderer(|r| r.free_font(id)).is_err() {
        with_headless_font_manager(|fonts| fonts.free(id));
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "measureText")]
pub fn measure_text(call: &mut ExternCallContext) -> ExternResult {
    let font_id = call.arg_u64(0) as u32;
    let text = call.arg_str(1);
    let size = call.arg_f64(2) as f32;
    let (w, h) = match with_renderer(|r| r.measure_text(font_id, text, size)) {
        Ok(result) => result,
        Err(_) => with_headless_font_manager(|fonts| fonts.measure_text(font_id, text, size)),
    };
    call.ret_f64(0, w as f64);
    call.ret_f64(1, h as f64);
    ExternResult::Ok
}

// --- Model externs ---

#[vo_fn("voplay", "loadModel")]
pub fn load_model(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    match with_renderer(|r| r.load_model(&path)) {
        Ok(Ok(id)) => {
            call.ret_u64(0, id as u64);
            write_nil_error(call, 1);
        }
        Ok(Err(msg)) | Err(msg) => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, &msg);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "loadModelBytes")]
pub fn load_model_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    match with_renderer(|r| r.load_model_bytes(&data)) {
        Ok(Ok(id)) => {
            call.ret_u64(0, id as u64);
            write_nil_error(call, 1);
        }
        Ok(Err(msg)) | Err(msg) => {
            call.ret_u64(0, 0);
            write_error_to(call, 1, &msg);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "freeModel")]
pub fn free_model(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_model(id));
    ExternResult::Ok
}
