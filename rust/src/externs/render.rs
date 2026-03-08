//! Surface init, frame submit, input poll, runtime query, native loop, and texture externs.

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use super::{RENDERER, with_renderer};
use crate::input;

// --- initSurface ---

#[vo_fn("voplay", "initSurface")]
pub fn init_surface(call: &mut ExternCallContext) -> ExternResult {
    let canvas_ref = call.arg_str(0).to_string();

    if RENDERER.get().is_some() {
        // Already initialized.
        write_nil_error(call, 0);
        return ExternResult::Ok;
    }

    #[cfg(feature = "wasm")]
    {
        match create_wasm_renderer(&canvas_ref) {
            Ok(()) => {
                write_nil_error(call, 0);
            }
            Err(msg) => {
                write_error_to(call, 0, &msg);
            }
        }
    }
    #[cfg(not(feature = "wasm"))]
    {
        let _ = canvas_ref;
        // Non-wasm, non-native: renderer can be injected via set_renderer().
        write_nil_error(call, 0);
    }

    ExternResult::Ok
}

#[cfg(feature = "wasm")]
fn create_wasm_renderer(canvas_id: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    use crate::renderer::Renderer;
    use std::sync::Mutex;

    let window = web_sys::window()
        .ok_or_else(|| "voplay: no global window".to_string())?;
    let document = window.document()
        .ok_or_else(|| "voplay: no document".to_string())?;
    let canvas = document.get_element_by_id(canvas_id)
        .ok_or_else(|| format!("voplay: canvas element '{}' not found", canvas_id))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| format!("voplay: element '{}' is not a canvas", canvas_id))?;

    let width = canvas.width();
    let height = canvas.height();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..Default::default()
    });

    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| format!("voplay: create_surface failed: {}", e))?;

    // wgpu adapter/device requests are async on WASM.
    // Spawn the async init; RENDERER will be set once complete.
    // submitFrame checks RENDERER availability each frame.
    wasm_bindgen_futures::spawn_local(async move {
        match Renderer::new(&instance, surface, width, height).await {
            Ok(renderer) => {
                let _ = RENDERER.set(Mutex::new(renderer));
                log::info!("voplay: WASM renderer initialized ({}x{})", width, height);
            }
            Err(msg) => {
                log::error!("voplay: WASM renderer init failed: {}", msg);
            }
        }
    });

    Ok(())
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
            write_error_to(call, 0, "voplay: renderer not initialized (call initSurface first)");
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

#[vo_fn("voplay", "nativePumpEvents")]
pub fn native_pump_events(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let (dt, closed) = crate::native::pump_events();
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

#[vo_fn("voplay", "nativeSubmitFrame")]
pub fn native_submit_frame(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let cmds = call.arg_bytes(0).to_vec();
        crate::native::submit_frame(cmds);
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "nativeWindowSize")]
pub fn native_window_size(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let (w, h) = crate::native::window_size();
        call.ret_u64(0, w as u64);
        call.ret_u64(1, h as u64);
    }
    #[cfg(not(feature = "native"))]
    {
        call.ret_u64(0, 0);
        call.ret_u64(1, 0);
    }
    ExternResult::Ok
}

// --- Texture externs ---

#[vo_fn("voplay", "loadTexture")]
pub fn load_texture(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    match with_renderer(|r| r.load_texture(&path)) {
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

#[vo_fn("voplay", "loadTextureBytes")]
pub fn load_texture_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    match with_renderer(|r| r.load_texture_bytes(&data)) {
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

#[vo_fn("voplay", "freeTexture")]
pub fn free_texture(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_texture(id));
    ExternResult::Ok
}

// --- runtimeIsWeb ---

#[vo_fn("voplay", "runtimeIsWeb")]
pub fn runtime_is_web(call: &mut ExternCallContext) -> ExternResult {
    let is_web = cfg!(target_arch = "wasm32");
    call.ret_bool(0, is_web);
    ExternResult::Ok
}

// --- isRendererReady ---

#[vo_fn("voplay", "isRendererReady")]
pub fn is_renderer_ready(call: &mut ExternCallContext) -> ExternResult {
    let ready = RENDERER.get().is_some();
    call.ret_bool(0, ready);
    ExternResult::Ok
}
