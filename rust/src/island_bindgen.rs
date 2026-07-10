//! wasm-bindgen island exports for voplay.
//!
//! Each function follows the ext_bridge tagged binary protocol:
//!
//! Input (one entry per param, in declaration order):
//!   Value  (int/uint/bool/float): [u64 LE — 8 bytes]
//!   Bytes  (string/[]byte):       [u32 LE len — 4 bytes][len bytes]
//!
//! Output (self-describing tagged stream):
//!   0xE0                           → nil error          (2 slots consumed)
//!   0xE1 [u16 LE len] [len bytes]  → error string       (2 slots consumed)
//!   0xE2 [u64 LE]                  → value              (1 slot)
//!   0xE3 [u32 LE len] [len bytes]  → bytes/string       (1 slot)
//!   0xE4                           → nil reference      (1 slot)
//!
//! Function names match what voCallExt extracts by stripping the module key prefix:
//!   extern "github_com_vo_lang_voplay_initSurface"  → wasm-bindgen export "initSurface"
//!   extern "github_com_vo_lang_voplay_scene2d_..."  → wasm-bindgen export "scene2d_..."
//!   extern "github_com_vo_lang_voplay_scene3d_..."  → wasm-bindgen export "scene3d_..."

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = "__voplayTakeWebGpuPerfPacket")]
    fn js_take_web_gpu_perf_packet() -> Result<js_sys::Uint8Array, JsValue>;
}

// ── Output tag constants ──────────────────────────────────────────────────────

const TAG_DISPLAY_PULSE: u8 = 0x03;
const TAG_NIL_ERROR: u8 = 0xE0;
const TAG_ERROR_STR: u8 = 0xE1;
const TAG_VALUE: u8 = 0xE2;
const TAG_BYTES: u8 = 0xE3;

// ── Output encoding helpers ───────────────────────────────────────────────────

#[inline]
fn out_nil_error(out: &mut Vec<u8>) {
    out.push(TAG_NIL_ERROR);
}

#[inline]
fn out_error(out: &mut Vec<u8>, msg: &str) {
    let bytes = msg.as_bytes();
    let len = bytes.len().min(65535) as u16;
    out.push(TAG_ERROR_STR);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

#[inline]
fn out_value_u64(out: &mut Vec<u8>, v: u64) {
    out.push(TAG_VALUE);
    out.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn out_value_f64(out: &mut Vec<u8>, v: f64) {
    out.push(TAG_VALUE);
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

#[inline]
fn out_value_bool(out: &mut Vec<u8>, b: bool) {
    out_value_u64(out, b as u64);
}

#[inline]
fn out_bytes(out: &mut Vec<u8>, data: &[u8]) {
    out.push(TAG_BYTES);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

/// Encode a Result<u32, String> as (TAG_VALUE u64)(nil_error|error_str)
#[inline]
fn out_u32_handle_result(out: &mut Vec<u8>, result: Result<u32, String>) {
    match result {
        Ok(id) => {
            out_value_u64(out, id as u64);
            out_nil_error(out);
        }
        Err(e) => {
            out_value_u64(out, 0);
            out_error(out, &e);
        }
    }
}

/// Encode a Result<(), String> as (nil_error|error_str)
#[inline]
fn out_unit_result(out: &mut Vec<u8>, result: Result<(), String>) {
    match result {
        Ok(()) => out_nil_error(out),
        Err(e) => out_error(out, &e),
    }
}

/// Encode a Result<bool, String> as (TAG_VALUE bool)(nil_error|error_str)
#[inline]
fn out_bool_result(out: &mut Vec<u8>, result: Result<bool, String>) {
    match result {
        Ok(value) => {
            out_value_bool(out, value);
            out_nil_error(out);
        }
        Err(e) => {
            out_value_bool(out, false);
            out_error(out, &e);
        }
    }
}

/// Encode a Result<Vec<u8>, String> as (TAG_BYTES)(nil_error|error_str)
#[inline]
fn out_bytes_result(out: &mut Vec<u8>, result: Result<Vec<u8>, String>) {
    match result {
        Ok(data) => {
            out_bytes(out, &data);
            out_nil_error(out);
        }
        Err(e) => {
            out_bytes(out, &[]);
            out_error(out, &e);
        }
    }
}

// ── Input decoding helpers ────────────────────────────────────────────────────

#[inline]
fn require_input(input: &[u8], pos: usize, needed: usize, field: &str) {
    let end = pos.checked_add(needed);
    if end.is_none() || end.unwrap_or(usize::MAX) > input.len() {
        panic!(
            "voplay: island input truncated field={field} offset={pos} needed={needed} remaining={}",
            input.len().saturating_sub(pos)
        );
    }
}

#[inline]
fn in_value(input: &[u8], pos: &mut usize) -> u64 {
    require_input(input, *pos, 8, "value");
    let start = *pos;
    let v = u64::from_le_bytes([
        input[start],
        input[start + 1],
        input[start + 2],
        input[start + 3],
        input[start + 4],
        input[start + 5],
        input[start + 6],
        input[start + 7],
    ]);
    *pos += 8;
    v
}

#[inline]
fn in_f64(input: &[u8], pos: &mut usize) -> f64 {
    f64::from_bits(in_value(input, pos))
}

#[inline]
fn in_bool(input: &[u8], pos: &mut usize) -> bool {
    in_value(input, pos) != 0
}

#[inline]
fn in_bytes<'a>(input: &'a [u8], pos: &mut usize) -> &'a [u8] {
    require_input(input, *pos, 4, "bytes.length");
    let start = *pos;
    let len = u32::from_le_bytes([
        input[start],
        input[start + 1],
        input[start + 2],
        input[start + 3],
    ]) as usize;
    *pos += 4;
    require_input(input, *pos, len, "bytes.payload");
    let data = &input[*pos..*pos + len];
    *pos += len;
    data
}

#[inline]
fn in_str<'a>(input: &'a [u8], pos: &mut usize) -> &'a str {
    let offset = *pos;
    let bytes = in_bytes(input, pos);
    match std::str::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => panic!(
            "voplay: island input invalid utf8 field=string offset={offset} valid_up_to={}",
            error.valid_up_to()
        ),
    }
}

// ── __voInit ──────────────────────────────────────────────────────────────────

/// Async GPU initialization hook. Called once by voSetupExtModule before first ext dispatch.
/// Renderer is initialized lazily on initSurface, so this is a no-op.
#[wasm_bindgen(js_name = "__voInit")]
pub fn vo_init() -> js_sys::Promise {
    console_error_panic_hook::set_once();
    js_sys::Promise::resolve(&JsValue::UNDEFINED)
}

#[wasm_bindgen(js_name = "__voDispose")]
pub fn vo_dispose() {
    crate::input::reset_wasm_input_handlers();
    let _ = crate::renderer_runtime::reset_renderer();
}

pub(crate) fn take_web_gpu_perf_packet_bridge() -> Result<Vec<u8>, String> {
    js_take_web_gpu_perf_packet()
        .map(|packet| packet.to_vec())
        .map_err(|_| "voplay: webgpu perf packet bridge failed".to_string())
}

mod animation;
mod physics2d;
mod physics3d;
mod render;
mod resource;
