//! Font and model load/free externs (voplay root package).

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use super::with_renderer;

// --- Font externs ---

#[vo_fn("voplay", "loadFont")]
pub fn load_font(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    match with_renderer(|r| r.load_font(&path)) {
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

#[vo_fn("voplay", "loadFontBytes")]
pub fn load_font_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    match with_renderer(|r| r.load_font_bytes(data)) {
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

#[vo_fn("voplay", "freeFont")]
pub fn free_font(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_font(id));
    ExternResult::Ok
}

#[vo_fn("voplay", "measureText")]
pub fn measure_text(call: &mut ExternCallContext) -> ExternResult {
    let font_id = call.arg_u64(0) as u32;
    let text = call.arg_str(1);
    let size = call.arg_f64(2) as f32;
    match with_renderer(|r| r.measure_text(font_id, text, size)) {
        Ok((w, h)) => {
            call.ret_f64(0, w as f64);
            call.ret_f64(1, h as f64);
        }
        Err(_) => {
            call.ret_f64(0, 0.0);
            call.ret_f64(1, 0.0);
        }
    }
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
