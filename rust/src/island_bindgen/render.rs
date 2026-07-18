use super::*;

// ── Render externs ────────────────────────────────────────────────────────────

/// initSurface(canvasRef string, noVsync bool) → error
#[vo_ext::vo_wasm_bindgen_export("voplay", "initSurface")]
pub fn init_surface(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let canvas_id = in_str(input, &mut pos).to_string();
    let no_vsync = in_bool(input, &mut pos);
    pos.finish();
    let result = crate::externs::render::create_wasm_renderer_pub(&canvas_id, no_vsync);
    let mut out = Vec::new();
    out_unit_result(&mut out, result);
    out
}

/// isRendererReady() → (bool, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "isRendererReady")]
pub fn is_renderer_ready(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let mut out = Vec::new();
    out_bool_result(&mut out, crate::externs::renderer_ready_result());
    out
}

/// submitFrame(cmds []byte) → error
#[vo_ext::vo_wasm_bindgen_export("voplay", "submitFrame")]
pub fn submit_frame(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let cmds = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::submit_renderer_frame(&cmds);
    let mut out = Vec::new();
    out_unit_result(&mut out, result);
    out
}

/// setRendererPerfStatsEnabled(enabled bool) → error
#[vo_ext::vo_wasm_bindgen_export("voplay", "setRendererPerfStatsEnabled")]
pub fn set_renderer_perf_stats_enabled(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let enabled = in_bool(input, &mut pos);
    pos.finish();
    let result = crate::externs::set_renderer_perf_stats_enabled(enabled);
    let mut out = Vec::new();
    out_unit_result(&mut out, result);
    out
}

/// lastRendererPerfPacket() → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "lastRendererPerfPacket")]
pub fn last_renderer_perf_packet(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let packet = crate::externs::last_renderer_perf_packet().unwrap_or_default();
    let mut out = Vec::new();
    out_bytes(&mut out, &packet);
    out
}

/// lastWebGpuPerfPacket() → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "lastWebGpuPerfPacket")]
pub fn last_web_gpu_perf_packet(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let packet = take_web_gpu_perf_packet_bridge().unwrap_or_default();
    let mut out = Vec::new();
    out_bytes(&mut out, &packet);
    out
}

/// pollInput() → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "pollInput")]
pub fn poll_input(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let events = crate::input::drain_input();
    let mut out = Vec::new();
    out_bytes(&mut out, &events);
    out
}

/// waitDisplayPulse()
#[vo_ext::vo_wasm_bindgen_export("voplay", "waitDisplayPulse")]
pub fn wait_display_pulse(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    vec![TAG_DISPLAY_PULSE]
}

/// loadTexture(path string) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTexture")]
pub fn load_texture(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture(&path));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureBytes(data []byte) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTextureBytes")]
pub fn load_texture_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture_bytes(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureRGBA(width uint32, height uint32, data []byte) -> (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTextureRGBA")]
pub fn load_texture_rgba(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let width = in_value(input, &mut pos) as u32;
    let height = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result =
        crate::externs::util::with_renderer_result(|r| r.load_texture_rgba(width, height, &data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureRGBALinear(width uint32, height uint32, data []byte) -> (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTextureRGBALinear")]
pub fn load_texture_rgba_linear(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let width = in_value(input, &mut pos) as u32;
    let height = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| {
        r.load_texture_rgba_linear(width, height, &data)
    });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureLinear(path string) -> (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTextureLinear")]
pub fn load_texture_linear(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture_linear(&path));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadTextureBytesLinear(data []byte) -> (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadTextureBytesLinear")]
pub fn load_texture_bytes_linear(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_texture_bytes_linear(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// texturePixelsBytes(id uint32) -> []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "texturePixelsBytes")]
pub fn texture_pixels_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let pixels = crate::externs::util::with_renderer_or_panic("texturePixelsBytes", |renderer| {
        renderer.texture_pixels(id)
    });
    let pixels = pixels.unwrap_or_else(|| panic!("texturePixelsBytes: texture not found: {}", id));
    let data = crate::externs::render::encode_texture_pixels_bytes(&pixels);
    let mut out = Vec::new();
    out_bytes(&mut out, &data);
    out
}

/// freeTexture(id uint32)
#[vo_ext::vo_wasm_bindgen_export("voplay", "freeTexture")]
pub fn free_texture(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let _ = crate::externs::with_renderer(|r| r.free_texture(id));
    Vec::new()
}

/// loadCubemap(right, left, top, bottom, front, back string) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadCubemap")]
pub fn load_cubemap(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let right = in_str(input, &mut pos).to_string();
    let left = in_str(input, &mut pos).to_string();
    let top = in_str(input, &mut pos).to_string();
    let bottom = in_str(input, &mut pos).to_string();
    let front = in_str(input, &mut pos).to_string();
    let back = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| {
        r.load_cubemap([&right, &left, &top, &bottom, &front, &back])
    });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadCubemapBytes(right, left, top, bottom, front, back []byte) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadCubemapBytes")]
pub fn load_cubemap_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let right = in_bytes(input, &mut pos).to_vec();
    let left = in_bytes(input, &mut pos).to_vec();
    let top = in_bytes(input, &mut pos).to_vec();
    let bottom = in_bytes(input, &mut pos).to_vec();
    let front = in_bytes(input, &mut pos).to_vec();
    let back = in_bytes(input, &mut pos).to_vec();
    pos.finish();
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
#[vo_ext::vo_wasm_bindgen_export("voplay", "freeCubemap")]
pub fn free_cubemap(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let _ = crate::externs::with_renderer(|r| r.free_cubemap(id));
    Vec::new()
}
