//! Vo extern implementations for voplay.
//! Registers: initSurface, submitFrame, pollInput, runtimeIsWeb.

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use std::sync::OnceLock;
use crate::renderer::Renderer;
use crate::input;

/// Global renderer instance, initialized by initSurface.
static RENDERER: OnceLock<std::sync::Mutex<Renderer>> = OnceLock::new();

#[allow(dead_code)]
fn get_renderer() -> &'static std::sync::Mutex<Renderer> {
    RENDERER.get().expect("voplay: renderer not initialized (call initSurface first)")
}

// --- initSurface ---

#[vo_fn("voplay", "initSurface")]
pub fn init_surface(call: &mut ExternCallContext) -> ExternResult {
    let _canvas_ref = call.arg_str(0).to_string();

    // Phase 0: For native, create a headless/dummy surface for testing.
    // Full surface creation requires platform-specific code (web: canvas element, native: winit).
    // For now, return an error explaining this is not yet implemented.
    // The actual implementation will be platform-specific in Phase 1.

    // TODO: Phase 1 — platform-specific surface creation
    //   Web: find <canvas> element, create wgpu surface
    //   Native: create winit window, create wgpu surface

    write_nil_error(call, 0);
    ExternResult::Ok
}

// --- submitFrame ---

#[vo_fn("voplay", "submitFrame")]
pub fn submit_frame(call: &mut ExternCallContext) -> ExternResult {
    let cmds = call.arg_bytes(0);

    match RENDERER.get() {
        Some(renderer_mutex) => {
            let mut renderer = renderer_mutex.lock().unwrap();
            match renderer.submit_frame(cmds) {
                Ok(()) => write_nil_error(call, 0),
                Err(msg) => write_error_to(call, 0, &msg),
            }
        }
        None => {
            // No renderer yet — silently succeed (Phase 0: no-op before surface init)
            write_nil_error(call, 0);
        }
    }

    ExternResult::Ok
}

// --- pollInput ---

#[vo_fn("voplay", "pollInput")]
pub fn poll_input(call: &mut ExternCallContext) -> ExternResult {
    let events = input::drain_input();
    let slice_ref = call.alloc_bytes(&events);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}

// --- float64 bit conversion (Vo lacks math.Float64bits/Float64frombits) ---

#[vo_fn("voplay", "float64Bits")]
pub fn float64_bits(call: &mut ExternCallContext) -> ExternResult {
    let f = call.arg_f64(0);
    call.ret_u64(0, f.to_bits());
    ExternResult::Ok
}

#[vo_fn("voplay", "float64FromBits")]
pub fn float64_from_bits(call: &mut ExternCallContext) -> ExternResult {
    let bits = call.arg_u64(0);
    call.ret_f64(0, f64::from_bits(bits));
    ExternResult::Ok
}

// --- Native game loop externs (only on native feature) ---

#[vo_fn("voplay", "nativeInit")]
pub fn native_init(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let width = call.arg_u64(0) as u32;
        let height = call.arg_u64(1) as u32;
        let title = call.arg_str(2).to_string();
        match crate::native::init(width, height, &title) {
            Ok(()) => write_nil_error(call, 0),
            Err(msg) => write_error_to(call, 0, &msg),
        }
    }
    #[cfg(not(feature = "native"))]
    {
        write_error_to(call, 0, "nativeInit not available on this platform");
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "nativeFrame")]
pub fn native_frame(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let cmds = call.arg_bytes(0).to_vec();
        let (dt, closed) = crate::native::frame(cmds);
        call.ret_f64(0, dt);
        call.ret_bool(1, closed);
    }
    #[cfg(not(feature = "native"))]
    {
        call.ret_f64(0, 0.0);
        call.ret_bool(1, true);
    }
    ExternResult::Ok
}

// --- runtimeIsWeb ---

#[vo_fn("voplay", "runtimeIsWeb")]
pub fn runtime_is_web(call: &mut ExternCallContext) -> ExternResult {
    let is_web = cfg!(target_arch = "wasm32");
    call.ret_bool(0, is_web);
    ExternResult::Ok
}
