//! Surface init, frame submit, input poll, runtime query, native loop, and texture externs.

use vo_ext::prelude::*;

use super::util::{ret_bytes, with_renderer_result, write_u32_handle_result, write_unit_result};
use super::{renderer_ready, submit_renderer_frame, with_renderer};
use crate::input;

// --- initSurface ---

#[vo_fn("voplay", "initSurface")]
pub fn init_surface(call: &mut ExternCallContext) -> ExternResult {
    let canvas_ref = call.arg_str(0).to_string();

    if renderer_ready() {
        // Already initialized.
        write_unit_result(call, 0, Ok(()));
        return ExternResult::Ok;
    }

    #[cfg(feature = "wasm")]
    {
        write_unit_result(call, 0, create_wasm_renderer(&canvas_ref));
    }
    #[cfg(not(feature = "wasm"))]
    {
        let _ = canvas_ref;
        // Non-wasm, non-native: renderer can be injected via set_renderer().
        write_unit_result(call, 0, Ok(()));
    }

    ExternResult::Ok
}

#[cfg(feature = "wasm")]
fn create_wasm_renderer(canvas_id: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    use crate::renderer::Renderer;

    let should_start = crate::renderer_runtime::begin_renderer_init()?;
    if !should_start {
        return Ok(());
    }

    let result: Result<(), String> = (|| {
        let window = web_sys::window()
            .ok_or_else(|| "voplay: no global window".to_string())?;
        let document = window.document()
            .ok_or_else(|| "voplay: no document".to_string())?;
        let canvas = document.get_element_by_id(canvas_id)
            .ok_or_else(|| format!("voplay: canvas element '{}' not found", canvas_id))?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| format!("voplay: element '{}' is not a canvas", canvas_id))?;

        crate::input::install_wasm_input_handlers(&canvas)?;

        let width = canvas.width();
        let height = canvas.height();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            ..Default::default()
        });

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| format!("voplay: create_surface failed: {}", e))?;

        wasm_bindgen_futures::spawn_local(async move {
            match Renderer::new(&instance, surface, width, height).await {
                Ok(renderer) => {
                    crate::renderer_runtime::set_renderer(renderer);
                    log::info!("voplay: WASM renderer initialized ({}x{})", width, height);
                }
                Err(msg) => {
                    crate::renderer_runtime::fail_renderer_init(msg.clone());
                    log::error!("voplay: WASM renderer init failed: {}", msg);
                }
            }
        });

        Ok(())
    })();

    if let Err(msg) = &result {
        crate::renderer_runtime::fail_renderer_init(msg.clone());
    }

    result
}

// --- submitFrame ---

#[vo_fn("voplay", "submitFrame")]
pub fn submit_frame(call: &mut ExternCallContext) -> ExternResult {
    let cmds = call.arg_bytes(0);

    let result = submit_renderer_frame(cmds);
    write_unit_result(call, 0, result);

    ExternResult::Ok
}

// --- pollInput ---

#[vo_fn("voplay", "pollInput")]
pub fn poll_input(call: &mut ExternCallContext) -> ExternResult {
    let events = input::drain_input();
    ret_bytes(call, 0, &events);
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
        write_unit_result(call, 0, crate::native::init(width, height, &title));
    }
    #[cfg(not(feature = "native"))]
    {
        write_unit_result(call, 0, Err("nativeInit not available on this platform".to_string()));
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
pub fn native_submit_frame(_call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let cmds = _call.arg_bytes(0).to_vec();
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
    write_u32_handle_result(call, 0, 1, with_renderer_result(|r| r.load_texture(&path)));
    ExternResult::Ok
}

#[vo_fn("voplay", "loadTextureBytes")]
pub fn load_texture_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    write_u32_handle_result(call, 0, 1, with_renderer_result(|r| r.load_texture_bytes(&data)));
    ExternResult::Ok
}

#[vo_fn("voplay", "freeTexture")]
pub fn free_texture(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_texture(id));
    ExternResult::Ok
}

#[vo_fn("voplay", "loadCubemap")]
pub fn load_cubemap(call: &mut ExternCallContext) -> ExternResult {
    let right = call.arg_str(0).to_string();
    let left = call.arg_str(1).to_string();
    let top = call.arg_str(2).to_string();
    let bottom = call.arg_str(3).to_string();
    let front = call.arg_str(4).to_string();
    let back = call.arg_str(5).to_string();
    write_u32_handle_result(
        call,
        0,
        1,
        with_renderer_result(|r| r.load_cubemap([&right, &left, &top, &bottom, &front, &back])),
    );
    ExternResult::Ok
}

#[vo_fn("voplay", "loadCubemapBytes")]
pub fn load_cubemap_bytes(call: &mut ExternCallContext) -> ExternResult {
    let right = call.arg_bytes(0).to_vec();
    let left = call.arg_bytes(1).to_vec();
    let top = call.arg_bytes(2).to_vec();
    let bottom = call.arg_bytes(3).to_vec();
    let front = call.arg_bytes(4).to_vec();
    let back = call.arg_bytes(5).to_vec();
    write_u32_handle_result(
        call,
        0,
        1,
        with_renderer_result(|r| {
            r.load_cubemap_bytes([
                right.as_slice(),
                left.as_slice(),
                top.as_slice(),
                bottom.as_slice(),
                front.as_slice(),
                back.as_slice(),
            ])
        }),
    );
    ExternResult::Ok
}

#[vo_fn("voplay", "freeCubemap")]
pub fn free_cubemap(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_cubemap(id));
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
    let ready = renderer_ready();
    call.ret_bool(0, ready);
    ExternResult::Ok
}
