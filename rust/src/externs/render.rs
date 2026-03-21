//! Surface init, frame submit, input poll, renderer query, and texture externs.

#[cfg(feature = "wasm")]
use core::sync::atomic::{AtomicU64, Ordering};
use vo_ext::prelude::*;

use super::util::{ret_bytes, with_renderer_result, write_u32_handle_result, write_unit_result};
use super::{renderer_ready, renderer_ready_result, submit_renderer_frame, with_renderer};
use crate::input;

#[cfg(feature = "wasm")]
static DISPLAY_PULSE_TOKEN: AtomicU64 = AtomicU64::new(1);

#[cfg(feature = "wasm")]
pub(crate) const DISPLAY_PULSE_DELAY_MS: u32 = u32::MAX;

#[cfg(feature = "wasm")]
fn next_display_pulse_token() -> u64 {
    DISPLAY_PULSE_TOKEN.fetch_add(1, Ordering::Relaxed)
}

// --- initSurface ---

#[vo_fn("voplay", "initSurface")]
pub fn init_surface(call: &mut ExternCallContext) -> ExternResult {
    let canvas_ref = call.arg_str(0).to_string();

    #[cfg(not(feature = "wasm"))]
    if renderer_ready() {
        write_unit_result(call, 0, Ok(()));
        return ExternResult::Ok;
    }

    #[cfg(feature = "wasm")]
    {
        write_unit_result(call, 0, create_wasm_renderer(&canvas_ref));
    }
    #[cfg(not(feature = "wasm"))]
    {
        write_unit_result(call, 0, create_native_renderer(&canvas_ref));
    }

    ExternResult::Ok
}

#[cfg(feature = "wasm")]
pub(crate) fn create_wasm_renderer_pub(canvas_id: &str) -> Result<(), String> {
    create_wasm_renderer(canvas_id)
}

#[cfg(feature = "wasm")]
fn create_wasm_renderer(canvas_id: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    use crate::renderer::Renderer;

    crate::input::reset_wasm_input_handlers();
    let generation = crate::renderer_runtime::reset_renderer();
    let should_start = crate::renderer_runtime::begin_renderer_init(generation)?;
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

        // Use CSS layout size × devicePixelRatio for the pixel buffer.
        // The HTML canvas default (300×150) is almost never correct.
        let dpr = window.device_pixel_ratio();
        let css_w = canvas.client_width().max(1) as f64;
        let css_h = canvas.client_height().max(1) as f64;
        let width = (css_w * dpr) as u32;
        let height = (css_h * dpr) as u32;
        canvas.set_width(width);
        canvas.set_height(height);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            ..Default::default()
        });

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| format!("voplay: create_surface failed: {}", e))?;

        let canvas_id_owned = canvas_id.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            match Renderer::new(&instance, surface, width, height).await {
                Ok(mut renderer) => {
                    renderer.set_canvas_id(canvas_id_owned);
                    crate::renderer_runtime::set_renderer_for_generation(generation, renderer);
                    log::info!("voplay: WASM renderer initialized ({}x{})", width, height);
                }
                Err(msg) => {
                    crate::renderer_runtime::fail_renderer_init(generation, msg.clone());
                    log::error!("voplay: WASM renderer init failed: {}", msg);
                }
            }
        });

        Ok(())
    })();

    if let Err(msg) = &result {
        crate::renderer_runtime::fail_renderer_init(generation, msg.clone());
    }

    result
}

#[cfg(not(feature = "wasm"))]
fn create_native_renderer(canvas_ref: &str) -> Result<(), String> {
    use std::ptr::NonNull;
    use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
    use crate::renderer::Renderer;

    let desc = crate::host_api::request_surface(canvas_ref)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let surface = match desc.kind {
        crate::host_api::SURFACE_KIND_APPKIT => {
            let ns_view = NonNull::new(desc.native_handle)
                .ok_or_else(|| format!("voplay: native host returned null AppKit view for '{}'", canvas_ref))?;
            let raw_window_handle = RawWindowHandle::AppKit(AppKitWindowHandle::new(ns_view));
            let raw_display_handle = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
            unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle,
                    raw_window_handle,
                })
            }
        }
        crate::host_api::SURFACE_KIND_CORE_ANIMATION_LAYER => {
            if desc.native_handle.is_null() {
                return Err(format!(
                    "voplay: native host returned null CoreAnimationLayer for '{}'",
                    canvas_ref,
                ));
            }
            unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(desc.native_handle))
            }
        }
        kind => {
            return Err(format!(
                "voplay: unsupported native surface kind {} for '{}'",
                kind,
                canvas_ref,
            ));
        }
    }
    .map_err(|e| format!("voplay: create_surface failed: {}", e))?;

    let renderer = pollster::block_on(Renderer::new(
        &instance,
        surface,
        desc.width.max(1),
        desc.height.max(1),
    ))?;
    crate::renderer_runtime::set_renderer(renderer);
    Ok(())
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

#[vo_fn("voplay", "waitDisplayPulse")]
pub fn wait_display_pulse(_call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "wasm")]
    {
        let token = next_display_pulse_token();
        return ExternResult::HostEventWait {
            token,
            delay_ms: DISPLAY_PULSE_DELAY_MS,
        };
    }

    #[cfg(not(feature = "wasm"))]
    {
        ExternResult::Ok
    }
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

// --- isRendererReady ---

#[vo_fn("voplay", "isRendererReady")]
pub fn is_renderer_ready(call: &mut ExternCallContext) -> ExternResult {
    let ready = renderer_ready_result().unwrap_or_else(|msg| panic!("{}", msg));
    call.ret_bool(0, ready);
    ExternResult::Ok
}

