//! Input event buffering for voplay.
//! Rust/JS side registers keyboard + pointer listeners, buffers events,
//! and returns packed binary via pollInput extern.

use std::sync::Mutex;

#[cfg(feature = "wasm")]
use std::cell::RefCell;
#[cfg(feature = "wasm")]
use wasm_bindgen::JsCast;
#[cfg(feature = "wasm")]
use wasm_bindgen::closure::Closure;
#[cfg(feature = "wasm")]
struct WasmInputHandlers {
    window: web_sys::Window,
    canvas: web_sys::HtmlCanvasElement,
    key_down: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    key_up: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    pointer_down: Closure<dyn FnMut(web_sys::PointerEvent)>,
    pointer_up: Closure<dyn FnMut(web_sys::PointerEvent)>,
    pointer_move: Closure<dyn FnMut(web_sys::PointerEvent)>,
    wheel: Closure<dyn FnMut(web_sys::WheelEvent)>,
}

#[cfg(feature = "wasm")]
thread_local! {
    static WASM_INPUT_HANDLERS: RefCell<Option<WasmInputHandlers>> = const { RefCell::new(None) };
}

/// Input event kinds (must match input.vo constants).
const INPUT_KEY_DOWN: u8 = 0x01;
const INPUT_KEY_UP: u8 = 0x02;
const INPUT_POINTER_DOWN: u8 = 0x03;
const INPUT_POINTER_UP: u8 = 0x04;
const INPUT_POINTER_MOVE: u8 = 0x05;
const INPUT_SCROLL: u8 = 0x06;

/// Global input buffer — events are appended by JS/native listeners,
/// drained by pollInput each frame.
static INPUT_BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Append a key event to the buffer.
pub fn push_key_event(down: bool, key_name: &str) {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    buf.push(if down { INPUT_KEY_DOWN } else { INPUT_KEY_UP });
    // name: u8 len + utf8
    let name_bytes = key_name.as_bytes();
    let len = name_bytes.len().min(255) as u8;
    buf.push(len);
    buf.extend_from_slice(&name_bytes[..len as usize]);
}

/// Append a pointer event to the buffer.
pub fn push_pointer_event(kind: u8, x: f64, y: f64, button: u8) {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    buf.push(kind);
    buf.extend_from_slice(&x.to_le_bytes());
    buf.extend_from_slice(&y.to_le_bytes());
    buf.push(button);
}

/// Append a scroll event to the buffer.
pub fn push_scroll_event(dx: f64, dy: f64) {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    buf.push(INPUT_SCROLL);
    buf.extend_from_slice(&dx.to_le_bytes());
    buf.extend_from_slice(&dy.to_le_bytes());
}

/// Drain the input buffer, returning all buffered events.
pub fn drain_input() -> Vec<u8> {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    std::mem::take(&mut *buf)
}

#[cfg(feature = "wasm")]
fn pointer_xy(canvas: &web_sys::HtmlCanvasElement, client_x: i32, client_y: i32) -> (f64, f64) {
    let rect = canvas.get_bounding_client_rect();
    (
        client_x as f64 - rect.left(),
        client_y as f64 - rect.top(),
    )
}

#[cfg(feature = "wasm")]
pub fn install_wasm_input_handlers(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
    let already_installed = WASM_INPUT_HANDLERS.with(|handlers| {
        let handlers = handlers.borrow();
        handlers.as_ref().is_some_and(|existing| existing.canvas.is_same_node(Some(canvas)))
    });
    if already_installed {
        return Ok(());
    }

    reset_wasm_input_handlers();

    let window = web_sys::window().ok_or_else(|| "voplay: no global window".to_string())?;

    let key_down = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
        let key = event.key();
        if !key.is_empty() {
            push_key_event(true, &key);
        }
    }) as Box<dyn FnMut(_)>);
    window
        .add_event_listener_with_callback("keydown", key_down.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register keydown listener".to_string())?;

    let key_up = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
        let key = event.key();
        if !key.is_empty() {
            push_key_event(false, &key);
        }
    }) as Box<dyn FnMut(_)>);
    window
        .add_event_listener_with_callback("keyup", key_up.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register keyup listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_down = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(POINTER_DOWN, x, y, event.button() as u8);
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("pointerdown", pointer_down.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointerdown listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_up = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(POINTER_UP, x, y, event.button() as u8);
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("pointerup", pointer_up.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointerup listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_move = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(POINTER_MOVE, x, y, 0);
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("pointermove", pointer_move.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointermove listener".to_string())?;

    let wheel = Closure::wrap(Box::new(move |event: web_sys::WheelEvent| {
        push_scroll_event(event.delta_x(), event.delta_y());
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register wheel listener".to_string())?;

    WASM_INPUT_HANDLERS.with(|handlers| {
        *handlers.borrow_mut() = Some(WasmInputHandlers {
            window,
            canvas: canvas.clone(),
            key_down,
            key_up,
            pointer_down,
            pointer_up,
            pointer_move,
            wheel,
        });
    });

    Ok(())
}

#[cfg(feature = "wasm")]
pub fn reset_wasm_input_handlers() {
    WASM_INPUT_HANDLERS.with(|handlers| {
        let Some(handlers) = handlers.borrow_mut().take() else {
            return;
        };

        let _ = handlers.window.remove_event_listener_with_callback(
            "keydown",
            handlers.key_down.as_ref().unchecked_ref(),
        );
        let _ = handlers.window.remove_event_listener_with_callback(
            "keyup",
            handlers.key_up.as_ref().unchecked_ref(),
        );
        let _ = handlers.canvas.remove_event_listener_with_callback(
            "pointerdown",
            handlers.pointer_down.as_ref().unchecked_ref(),
        );
        let _ = handlers.canvas.remove_event_listener_with_callback(
            "pointerup",
            handlers.pointer_up.as_ref().unchecked_ref(),
        );
        let _ = handlers.canvas.remove_event_listener_with_callback(
            "pointermove",
            handlers.pointer_move.as_ref().unchecked_ref(),
        );
        let _ = handlers.canvas.remove_event_listener_with_callback(
            "wheel",
            handlers.wheel.as_ref().unchecked_ref(),
        );
    });
    INPUT_BUFFER.lock().unwrap().clear();
}

/// Convenience constants for pointer events (re-exported for platform modules).
pub const POINTER_DOWN: u8 = INPUT_POINTER_DOWN;
pub const POINTER_UP: u8 = INPUT_POINTER_UP;
pub const POINTER_MOVE: u8 = INPUT_POINTER_MOVE;
#[allow(dead_code)]
pub const SCROLL: u8 = INPUT_SCROLL;
