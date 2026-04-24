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

#[cfg(feature = "wasm")]
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "wasm")]
static POLL_INPUT_DEBUG_COUNT: AtomicU32 = AtomicU32::new(0);

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = warn)]
    fn console_warn(message: &str);
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
fn in_value(input: &[u8], pos: &mut usize) -> u64 {
    let v = u64::from_le_bytes(input[*pos..*pos + 8].try_into().unwrap());
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
    let len = u32::from_le_bytes(input[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let data = &input[*pos..*pos + len];
    *pos += len;
    data
}

#[inline]
fn in_str<'a>(input: &'a [u8], pos: &mut usize) -> &'a str {
    let bytes = in_bytes(input, pos);
    std::str::from_utf8(bytes).unwrap_or("")
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

// ── Render externs ────────────────────────────────────────────────────────────

/// initSurface(canvasRef string, noVsync bool) → error
#[wasm_bindgen(js_name = "initSurface")]
pub fn init_surface(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let canvas_id = in_str(input, &mut pos).to_string();
    let no_vsync = in_bool(input, &mut pos);
    let result = crate::externs::render::create_wasm_renderer_pub(&canvas_id, no_vsync);
    let mut out = Vec::new();
    out_unit_result(&mut out, result);
    out
}

/// isRendererReady() → bool
#[wasm_bindgen(js_name = "isRendererReady")]
pub fn is_renderer_ready(_input: &[u8]) -> Vec<u8> {
    let ready = crate::externs::renderer_ready_result().unwrap_or_else(|msg| panic!("{}", msg));
    let mut out = Vec::new();
    out_value_bool(&mut out, ready);
    out
}

/// submitFrame(cmds []byte) → error
#[wasm_bindgen(js_name = "submitFrame")]
pub fn submit_frame(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let cmds = in_bytes(input, &mut pos).to_vec();
    let result = crate::externs::submit_renderer_frame(&cmds);
    let mut out = Vec::new();
    out_unit_result(&mut out, result);
    out
}

/// pollInput() → []byte
#[wasm_bindgen(js_name = "pollInput")]
pub fn poll_input(_input: &[u8]) -> Vec<u8> {
    let events = crate::input::drain_input();
    let seen = POLL_INPUT_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed);
    if seen < 24 || !events.is_empty() {
        console_warn(&format!("[voplay island] pollInput bytes={}", events.len()));
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &events);
    out
}

/// waitDisplayPulse()
#[wasm_bindgen(js_name = "waitDisplayPulse")]
pub fn wait_display_pulse(_input: &[u8]) -> Vec<u8> {
    vec![TAG_DISPLAY_PULSE]
}

/// loadTexture(path string) → (uint32, error)
#[wasm_bindgen(js_name = "loadTexture")]
pub fn load_texture(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture(&path));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureBytes(data []byte) → (uint32, error)
#[wasm_bindgen(js_name = "loadTextureBytes")]
pub fn load_texture_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let data = in_bytes(input, &mut pos).to_vec();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture_bytes(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeTexture(id uint32)
#[wasm_bindgen(js_name = "freeTexture")]
pub fn free_texture(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    let _ = crate::externs::with_renderer(|r| r.free_texture(id));
    Vec::new()
}

/// loadCubemap(right, left, top, bottom, front, back string) → (uint32, error)
#[wasm_bindgen(js_name = "loadCubemap")]
pub fn load_cubemap(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let right = in_str(input, &mut pos).to_string();
    let left = in_str(input, &mut pos).to_string();
    let top = in_str(input, &mut pos).to_string();
    let bottom = in_str(input, &mut pos).to_string();
    let front = in_str(input, &mut pos).to_string();
    let back = in_str(input, &mut pos).to_string();
    let result = crate::externs::util::with_renderer_result(|r| {
        r.load_cubemap([&right, &left, &top, &bottom, &front, &back])
    });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadCubemapBytes(right, left, top, bottom, front, back []byte) → (uint32, error)
#[wasm_bindgen(js_name = "loadCubemapBytes")]
pub fn load_cubemap_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let right = in_bytes(input, &mut pos).to_vec();
    let left = in_bytes(input, &mut pos).to_vec();
    let top = in_bytes(input, &mut pos).to_vec();
    let bottom = in_bytes(input, &mut pos).to_vec();
    let front = in_bytes(input, &mut pos).to_vec();
    let back = in_bytes(input, &mut pos).to_vec();
    let result = crate::externs::util::with_renderer_result(|r| {
        r.load_cubemap_bytes([
            right.as_slice(),
            left.as_slice(),
            top.as_slice(),
            bottom.as_slice(),
            front.as_slice(),
            back.as_slice(),
        ])
    });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeCubemap(id uint32)
#[wasm_bindgen(js_name = "freeCubemap")]
pub fn free_cubemap(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    let _ = crate::externs::with_renderer(|r| r.free_cubemap(id));
    Vec::new()
}

// ── Resource externs ──────────────────────────────────────────────────────────

/// loadFont(path string) → (uint32, error)
#[wasm_bindgen(js_name = "loadFont")]
pub fn load_font(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let result = match crate::externs::with_renderer(|r| r.load_font(&path)) {
        Ok(result) => result,
        Err(_) => res::with_headless_font_manager_pub(|fonts| fonts.load_file(&path))
            .and_then(|result| result),
    };
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadFontBytes(data []byte) → (uint32, error)
#[wasm_bindgen(js_name = "loadFontBytes")]
pub fn load_font_bytes(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = 0usize;
    let data = in_bytes(input, &mut pos).to_vec();
    let result = match crate::externs::with_renderer(|r| r.load_font_bytes(data.clone())) {
        Ok(result) => result,
        Err(_) => res::with_headless_font_manager_pub(|fonts| fonts.load_bytes(data))
            .and_then(|result| result),
    };
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeFont(id uint32)
#[wasm_bindgen(js_name = "freeFont")]
pub fn free_font(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    if crate::externs::with_renderer(|r| r.free_font(id)).is_err() {
        res::with_headless_font_manager_pub(|fonts| fonts.free(id))
            .unwrap_or_else(|msg| panic!("{}", msg));
    }
    Vec::new()
}

/// measureText(fontId uint32, text string, size float) → (float, float)
#[wasm_bindgen(js_name = "measureText")]
pub fn measure_text(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = 0usize;
    let font_id = in_value(input, &mut pos) as u32;
    let text = in_str(input, &mut pos).to_string();
    let size = in_f64(input, &mut pos) as f32;
    let (w, h) = match crate::externs::with_renderer(|r| r.measure_text(font_id, &text, size)) {
        Ok(result) => result,
        Err(_) => {
            res::with_headless_font_manager_pub(|fonts| fonts.measure_text(font_id, &text, size))
                .unwrap_or_else(|msg| panic!("{}", msg))
        }
    };
    let mut out = Vec::new();
    out_value_f64(&mut out, w as f64);
    out_value_f64(&mut out, h as f64);
    out
}

/// loadModel(path string) → (uint32, error)
#[wasm_bindgen(js_name = "loadModel")]
pub fn load_model(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let result = crate::externs::util::with_renderer_result(|r| r.load_model(&path));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadModelBytes(data []byte) → (uint32, error)
#[wasm_bindgen(js_name = "loadModelBytes")]
pub fn load_model_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let data = in_bytes(input, &mut pos).to_vec();
    let result = crate::externs::util::with_renderer_result(|r| r.load_model_bytes(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeModel(id uint32)
#[wasm_bindgen(js_name = "freeModel")]
pub fn free_model(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    let _ = crate::externs::with_renderer(|r| r.free_model(id));
    Vec::new()
}

/// modelBounds(id uint32) → (float, float, float, float, float, float, bool)
#[wasm_bindgen(js_name = "modelBounds")]
pub fn model_bounds(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    let mut out = Vec::new();
    match crate::externs::with_renderer(|r| r.model_bounds(id)) {
        Ok(Some((min, max))) => {
            out_value_f64(&mut out, min[0] as f64);
            out_value_f64(&mut out, min[1] as f64);
            out_value_f64(&mut out, min[2] as f64);
            out_value_f64(&mut out, max[0] as f64);
            out_value_f64(&mut out, max[1] as f64);
            out_value_f64(&mut out, max[2] as f64);
            out_value_bool(&mut out, true);
        }
        _ => {
            for _ in 0..6 {
                out_value_f64(&mut out, 0.0);
            }
            out_value_bool(&mut out, false);
        }
    }
    out
}

/// modelMeshDataBytes(id uint32) → []byte
#[wasm_bindgen(js_name = "modelMeshDataBytes")]
pub fn model_mesh_data_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let id = in_value(input, &mut pos) as u32;
    let mesh_data =
        crate::externs::util::with_renderer_or_panic("modelMeshDataBytes", |renderer| {
            renderer.get_model_mesh_data(id)
        });
    let (positions, indices) =
        mesh_data.unwrap_or_else(|| panic!("modelMeshDataBytes: model not found: {}", id));
    let data = crate::externs::resource::encode_model_mesh_data_bytes(&positions, &indices);
    let mut out = Vec::new();
    out_bytes(&mut out, &data);
    out
}

// ── scene3d resource externs ──────────────────────────────────────────────────

/// scene3d_loadLevel(path string) → ([]byte, error)
#[wasm_bindgen(js_name = "scene3d_loadLevel")]
pub fn scene3d_load_level(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let result = crate::externs::util::with_renderer_result(|r| r.load_level(&path))
        .map(|nodes| crate::externs::resource::serialize_level_nodes(&nodes));
    let mut out = Vec::new();
    out_bytes_result(&mut out, result);
    out
}

/// scene3d_createTerrain(path, sx, sy, sz, uvScale, texId) → terrain result
#[wasm_bindgen(js_name = "scene3d_createTerrain")]
pub fn scene3d_create_terrain(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let uv_scale = in_f64(input, &mut pos) as f32;
    let texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let result = crate::file_io::read_bytes(&path)
        .map_err(|e| format!("terrain: read {}: {}", path, e))
        .and_then(|data| {
            crate::externs::util::with_renderer_result(|r| {
                r.create_terrain(&data, scale_x, scale_y, scale_z, uv_scale, texture_id)
            })
        });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainSplat(path, sx, sy, sz, controlTexId, [tex, uv]*4) → terrain result
#[wasm_bindgen(js_name = "scene3d_createTerrainSplat")]
pub fn scene3d_create_terrain_splat(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let (control_texture_id, layer_texture_ids, uv_scales) =
        decode_terrain_splat_input(input, &mut pos);
    let result = crate::file_io::read_bytes(&path)
        .map_err(|e| format!("terrain: read {}: {}", path, e))
        .and_then(|data| {
            crate::externs::util::with_renderer_result(|r| {
                r.create_terrain_splat(
                    &data,
                    scale_x,
                    scale_y,
                    scale_z,
                    control_texture_id,
                    layer_texture_ids,
                    uv_scales,
                )
            })
        });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainBytes(data []byte, sx, sy, sz, uvScale, texId) → terrain result
#[wasm_bindgen(js_name = "scene3d_createTerrainBytes")]
pub fn scene3d_create_terrain_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let data = in_bytes(input, &mut pos).to_vec();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let uv_scale = in_f64(input, &mut pos) as f32;
    let texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let result = crate::externs::util::with_renderer_result(|r| {
        r.create_terrain(&data, scale_x, scale_y, scale_z, uv_scale, texture_id)
    });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainBytesSplat(data, sx, sy, sz, controlTexId, [tex, uv]*4) → terrain result
#[wasm_bindgen(js_name = "scene3d_createTerrainBytesSplat")]
pub fn scene3d_create_terrain_bytes_splat(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let data = in_bytes(input, &mut pos).to_vec();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let (control_texture_id, layer_texture_ids, uv_scales) =
        decode_terrain_splat_input(input, &mut pos);
    let result = crate::externs::util::with_renderer_result(|r| {
        r.create_terrain_splat(
            &data,
            scale_x,
            scale_y,
            scale_z,
            control_texture_id,
            layer_texture_ids,
            uv_scales,
        )
    });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

fn decode_terrain_splat_input(input: &[u8], pos: &mut usize) -> (u32, [u32; 4], [f32; 4]) {
    let control_texture_id = in_value(input, pos) as u32;
    let mut layer_texture_ids = [0u32; 4];
    let mut uv_scales = [1.0f32; 4];
    for i in 0..4 {
        layer_texture_ids[i] = in_value(input, pos) as u32;
        uv_scales[i] = in_f64(input, pos) as f32;
    }
    (control_texture_id, layer_texture_ids, uv_scales)
}

/// scene3d_terrainHeightAt(worldId, bodyId, x, z) → (float, bool)
#[wasm_bindgen(js_name = "scene3d_terrainHeightAt")]
pub fn scene3d_terrain_height_at(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let x = in_f64(input, &mut pos) as f32;
    let z = in_f64(input, &mut pos) as f32;
    let mut out = Vec::new();
    match crate::terrain::height_at(world_id, body_id, x, z) {
        Some(h) => {
            out_value_f64(&mut out, h as f64);
            out_value_bool(&mut out, true);
        }
        None => {
            out_value_f64(&mut out, 0.0);
            out_value_bool(&mut out, false);
        }
    }
    out
}

/// scene3d_createPlaneMesh(width, depth, subX, subZ) → uint32
#[wasm_bindgen(js_name = "createPlaneMesh")]
pub fn scene3d_create_plane_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let width = in_f64(input, &mut pos) as f32;
    let depth = in_f64(input, &mut pos) as f32;
    let sub_x = in_value(input, &mut pos) as u32;
    let sub_z = in_value(input, &mut pos) as u32;
    let id = crate::externs::util::with_renderer_or_panic("createPlaneMesh", |r| {
        r.create_plane(width, depth, sub_x, sub_z)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCubeMesh() → uint32
#[wasm_bindgen(js_name = "createCubeMesh")]
pub fn scene3d_create_cube_mesh(_input: &[u8]) -> Vec<u8> {
    let id = crate::externs::util::with_renderer_or_panic("createCubeMesh", |r| r.create_cube());
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createSphereMesh(segments) → uint32
#[wasm_bindgen(js_name = "createSphereMesh")]
pub fn scene3d_create_sphere_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let segments = in_value(input, &mut pos) as u32;
    let id = crate::externs::util::with_renderer_or_panic("createSphereMesh", |r| {
        r.create_sphere(segments)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCylinderMesh(segments) → uint32
#[wasm_bindgen(js_name = "createCylinderMesh")]
pub fn scene3d_create_cylinder_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let segments = in_value(input, &mut pos) as u32;
    let id = crate::externs::util::with_renderer_or_panic("createCylinderMesh", |r| {
        r.create_cylinder(segments)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCapsuleMesh(segments, halfHeight, radius) → uint32
#[wasm_bindgen(js_name = "createCapsuleMesh")]
pub fn scene3d_create_capsule_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let segments = in_value(input, &mut pos) as u32;
    let half_height = in_f64(input, &mut pos) as f32;
    let radius = in_f64(input, &mut pos) as f32;
    let id = crate::externs::util::with_renderer_or_panic("createCapsuleMesh", |r| {
        r.create_capsule(segments, half_height, radius)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

// ── Audio externs ─────────────────────────────────────────────────────────────

/// audioLoadFile(path string) → (uint32, error)
#[wasm_bindgen(js_name = "audioLoadFile")]
pub fn audio_load_file(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let path = in_str(input, &mut pos).to_string();
    let result = crate::file_io::read_bytes(&path)
        .map_err(|e| format!("audio load error: {e}"))
        .and_then(|data| {
            vo_vogui::audio::with_global_audio_result(|engine| engine.load_bytes(data))
        });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

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
    use std::collections::HashMap;
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

// ── scene2d physics externs ───────────────────────────────────────────────────

/// scene2d_physicsInit(gx, gy) → uint32
#[wasm_bindgen(js_name = "scene2d_physicsInit")]
pub fn scene2d_physics_init(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    let world_id = crate::physics::create_world(gx, gy);
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// scene2d_physicsDestroy(worldId)
#[wasm_bindgen(js_name = "scene2d_physicsDestroy")]
pub fn scene2d_physics_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    crate::physics::destroy_world(world_id);
    Vec::new()
}

/// scene2d_physicsSpawnBody(worldId, bodyId, data []byte)
#[wasm_bindgen(js_name = "scene2d_physicsSpawnBody")]
pub fn scene2d_physics_spawn_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let desc = crate::externs::physics2d::decode_body_desc(body_id, data);
    crate::physics::with_world(world_id, |world| world.spawn_body(&desc));
    Vec::new()
}

/// scene2d_physicsDestroyBody(worldId, bodyId)
#[wasm_bindgen(js_name = "scene2d_physicsDestroyBody")]
pub fn scene2d_physics_destroy_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    crate::physics::with_world(world_id, |world| world.destroy_body(body_id));
    Vec::new()
}

/// scene2d_physicsStep(worldId, dt, cmds []byte) → []byte
#[wasm_bindgen(js_name = "scene2d_physicsStep")]
pub fn scene2d_physics_step(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let cmds = in_bytes(input, &mut pos).to_vec();
    let state = crate::physics::with_world(world_id, |world| {
        world.apply_commands(&cmds);
        world.step(dt);
        world.serialize_state()
    });
    let mut out = Vec::new();
    out_bytes(&mut out, &state);
    out
}

/// scene2d_physicsSetGravity(worldId, gx, gy)
#[wasm_bindgen(js_name = "scene2d_physicsSetGravity")]
pub fn scene2d_physics_set_gravity(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    crate::physics::with_world(world_id, |world| world.set_gravity(gx, gy));
    Vec::new()
}

/// scene2d_physicsContacts(worldId) → []byte
#[wasm_bindgen(js_name = "scene2d_physicsContacts")]
pub fn scene2d_physics_contacts(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let contacts = crate::physics::with_world(world_id, |world| world.get_contacts());
    let mut buf = Vec::with_capacity(4 + contacts.len() * 8);
    buf.extend_from_slice(&(contacts.len() as u32).to_le_bytes());
    for (a, b) in &contacts {
        buf.extend_from_slice(&a.to_le_bytes());
        buf.extend_from_slice(&b.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene2d_physicsRayCast(worldId, ox, oy, dx, dy, maxDist) → []byte
#[wasm_bindgen(js_name = "scene2d_physicsRayCast")]
pub fn scene2d_physics_ray_cast(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let ox = in_f64(input, &mut pos) as f32;
    let oy = in_f64(input, &mut pos) as f32;
    let dx = in_f64(input, &mut pos) as f32;
    let dy = in_f64(input, &mut pos) as f32;
    let max_dist = in_f64(input, &mut pos) as f32;
    let result =
        crate::physics::with_world(world_id, |world| world.ray_cast(ox, oy, dx, dy, max_dist));
    let buf = match result {
        Some((body_id, hx, hy, nx, ny, toi)) => {
            let mut b = Vec::with_capacity(45);
            b.push(1u8);
            b.extend_from_slice(&body_id.to_le_bytes());
            b.extend_from_slice(&(hx as f64).to_le_bytes());
            b.extend_from_slice(&(hy as f64).to_le_bytes());
            b.extend_from_slice(&(nx as f64).to_le_bytes());
            b.extend_from_slice(&(ny as f64).to_le_bytes());
            b.extend_from_slice(&(toi as f64).to_le_bytes());
            b
        }
        None => vec![0u8],
    };
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene2d_physicsQueryRect(worldId, minX, minY, maxX, maxY) → []byte
#[wasm_bindgen(js_name = "scene2d_physicsQueryRect")]
pub fn scene2d_physics_query_rect(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let min_x = in_f64(input, &mut pos) as f32;
    let min_y = in_f64(input, &mut pos) as f32;
    let max_x = in_f64(input, &mut pos) as f32;
    let max_y = in_f64(input, &mut pos) as f32;
    let ids = crate::physics::with_world(world_id, |world| {
        world.query_rect(min_x, min_y, max_x, max_y)
    });
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

// ── scene3d physics externs ───────────────────────────────────────────────────

/// scene3d_physicsInit(gx, gy, gz) → uint32
#[wasm_bindgen(js_name = "scene3d_physicsInit")]
pub fn scene3d_physics_init(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    let gz = in_f64(input, &mut pos) as f32;
    let world_id = crate::physics3d::create_world(gx, gy, gz);
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// scene3d_physicsDestroy(worldId)
#[wasm_bindgen(js_name = "scene3d_physicsDestroy")]
pub fn scene3d_physics_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    crate::terrain::remove_world(world_id);
    crate::physics3d::destroy_world(world_id);
    Vec::new()
}

/// scene3d_physicsSpawnBody(worldId, bodyId, data []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnBody")]
pub fn scene3d_physics_spawn_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let desc = crate::externs::physics3d::decode_body3d_desc(body_id, data);
    crate::physics3d::with_world(world_id, |world| world.spawn_body(&desc));
    Vec::new()
}

/// scene3d_physicsSpawnTrimeshBody(worldId, bodyId, modelId, data []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnTrimeshBody")]
pub fn scene3d_physics_spawn_trimesh_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let model_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let desc = crate::externs::physics3d::decode_trimesh_desc(body_id, data);
    let mesh_data = crate::externs::util::with_renderer_or_panic("physicsSpawnTrimeshBody", |r| {
        r.get_model_mesh_data(model_id)
    });
    let (positions, indices) = mesh_data
        .unwrap_or_else(|| panic!("physicsSpawnTrimeshBody: model not found: {}", model_id));
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_trimesh_body(&desc, &positions, &indices)
    });
    Vec::new()
}

/// scene3d_physicsSpawnTrimeshBodyData(worldId, bodyId, data []byte, meshData []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnTrimeshBodyData")]
pub fn scene3d_physics_spawn_trimesh_body_data(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let mesh_data = in_bytes(input, &mut pos);
    crate::externs::physics3d::spawn_trimesh_body_from_mesh_data(
        world_id, body_id, data, mesh_data,
    );
    Vec::new()
}

/// scene3d_physicsSpawnHeightfield(worldId, bodyId, heights []byte, rows, cols, sx, sy, sz, px, py, pz, layer, mask, friction, restitution)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnHeightfield")]
pub fn scene3d_physics_spawn_heightfield(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let height_bytes = in_bytes(input, &mut pos);
    let rows = in_value(input, &mut pos) as u32;
    let cols = in_value(input, &mut pos) as u32;
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let px = in_f64(input, &mut pos) as f32;
    let py = in_f64(input, &mut pos) as f32;
    let pz = in_f64(input, &mut pos) as f32;
    let layer = in_value(input, &mut pos) as u16;
    let mask = in_value(input, &mut pos) as u16;
    let friction = in_f64(input, &mut pos) as f32;
    let restitution = in_f64(input, &mut pos) as f32;

    let height_data: Vec<f32> = height_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let origin = Vec3::new(px, py, pz);
    let desc = crate::physics3d::HeightfieldDesc3D {
        body_id,
        pos: origin,
        layer,
        mask,
        friction,
        restitution,
        rows,
        cols,
        scale_x,
        scale_y,
        scale_z,
    };
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_heightfield_body(&desc, &height_data)
    });
    crate::terrain::store_terrain(
        world_id,
        body_id,
        origin,
        crate::terrain::TerrainData {
            model_id: 0,
            heights: height_data,
            rows,
            cols,
            scale_x,
            scale_y,
            scale_z,
            origin,
        },
    );
    Vec::new()
}

/// scene3d_physicsDestroyBody(worldId, bodyId)
#[wasm_bindgen(js_name = "scene3d_physicsDestroyBody")]
pub fn scene3d_physics_destroy_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    crate::terrain::remove_terrain(world_id, body_id);
    crate::physics3d::with_world(world_id, |world| world.destroy_body(body_id));
    Vec::new()
}

/// scene3d_physicsStep(worldId, dt, cmds []byte) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsStep")]
pub fn scene3d_physics_step(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let cmds = in_bytes(input, &mut pos).to_vec();
    let state = crate::physics3d::with_world(world_id, |world| {
        world.apply_commands(&cmds);
        world.step(dt);
        world.serialize_state()
    });
    let mut out = Vec::new();
    out_bytes(&mut out, &state);
    out
}

/// scene3d_physicsSetGravity(worldId, gx, gy, gz)
#[wasm_bindgen(js_name = "scene3d_physicsSetGravity")]
pub fn scene3d_physics_set_gravity(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    let gz = in_f64(input, &mut pos) as f32;
    crate::physics3d::with_world(world_id, |world| world.set_gravity(gx, gy, gz));
    Vec::new()
}

/// scene3d_physicsContacts(worldId) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsContacts")]
pub fn scene3d_physics_contacts(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let contacts = crate::physics3d::with_world(world_id, |world| world.get_contacts());
    let mut buf = Vec::with_capacity(4 + contacts.len() * 8);
    buf.extend_from_slice(&(contacts.len() as u32).to_le_bytes());
    for (a, b) in &contacts {
        buf.extend_from_slice(&a.to_le_bytes());
        buf.extend_from_slice(&b.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene3d_physicsRayCast(worldId, ox, oy, oz, dx, dy, dz, maxDist) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsRayCast")]
pub fn scene3d_physics_ray_cast(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let ox = in_f64(input, &mut pos) as f32;
    let oy = in_f64(input, &mut pos) as f32;
    let oz = in_f64(input, &mut pos) as f32;
    let dx = in_f64(input, &mut pos) as f32;
    let dy = in_f64(input, &mut pos) as f32;
    let dz = in_f64(input, &mut pos) as f32;
    let max_dist = in_f64(input, &mut pos) as f32;
    let origin = Vec3::new(ox, oy, oz);
    let dir = Vec3::new(dx, dy, dz);
    let result =
        crate::physics3d::with_world(world_id, |world| world.ray_cast(origin, dir, max_dist));
    let buf = match result {
        Some(hit) => {
            let mut b = Vec::with_capacity(61);
            b.push(1u8);
            b.extend_from_slice(&hit.body_id.to_le_bytes());
            b.extend_from_slice(&(hit.point.x as f64).to_le_bytes());
            b.extend_from_slice(&(hit.point.y as f64).to_le_bytes());
            b.extend_from_slice(&(hit.point.z as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.x as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.y as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.z as f64).to_le_bytes());
            b.extend_from_slice(&(hit.toi as f64).to_le_bytes());
            b
        }
        None => vec![0u8],
    };
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene3d_physicsQueryAABB(worldId, minX, minY, minZ, maxX, maxY, maxZ) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsQueryAABB")]
pub fn scene3d_physics_query_aabb(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let min_x = in_f64(input, &mut pos) as f32;
    let min_y = in_f64(input, &mut pos) as f32;
    let min_z = in_f64(input, &mut pos) as f32;
    let max_x = in_f64(input, &mut pos) as f32;
    let max_y = in_f64(input, &mut pos) as f32;
    let max_z = in_f64(input, &mut pos) as f32;
    let min = Vec3::new(min_x, min_y, min_z);
    let max = Vec3::new(max_x, max_y, max_z);
    let ids = crate::physics3d::with_world(world_id, |world| world.query_aabb(min, max));
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}
