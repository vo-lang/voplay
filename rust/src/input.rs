//! Input event buffering for voplay.
//! Rust/JS side registers keyboard + pointer listeners, buffers events,
//! and returns packed binary via pollInput extern.

use std::sync::Mutex;

#[cfg(feature = "wasm")]
use std::cell::RefCell;
#[cfg(feature = "wasm")]
use wasm_bindgen::closure::Closure;
#[cfg(feature = "wasm")]
use wasm_bindgen::JsCast;

#[cfg(feature = "wasm")]
struct WasmInputHandlers {
    canvas: web_sys::HtmlCanvasElement,
    document: web_sys::Document,
    key_down: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    key_up: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    pointer_down: Closure<dyn FnMut(web_sys::PointerEvent)>,
    pointer_up: Closure<dyn FnMut(web_sys::PointerEvent)>,
    pointer_move: Closure<dyn FnMut(web_sys::PointerEvent)>,
    pointer_cancel: Closure<dyn FnMut(web_sys::PointerEvent)>,
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
#[cfg(feature = "wasm")]
static ACTIVE_POINTER_IDS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

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
pub fn push_pointer_event(kind: u8, id: u32, x: f64, y: f64, button: u8) {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    buf.push(kind);
    buf.extend_from_slice(&id.to_le_bytes());
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
    let scale_x = if rect.width() > 0.0 {
        canvas.client_width().max(1) as f64 / rect.width()
    } else {
        1.0
    };
    let scale_y = if rect.height() > 0.0 {
        canvas.client_height().max(1) as f64 / rect.height()
    } else {
        1.0
    };
    (
        (client_x as f64 - rect.left()) * scale_x,
        (client_y as f64 - rect.top()) * scale_y,
    )
}

#[cfg(feature = "wasm")]
fn pointer_button(event: &web_sys::PointerEvent) -> u8 {
    event.button().clamp(0, 255) as u8
}

#[cfg(feature = "wasm")]
fn pointer_id(event: &web_sys::PointerEvent) -> u32 {
    event.pointer_id().max(0) as u32
}

#[cfg(feature = "wasm")]
fn pointer_id_raw(event: &web_sys::PointerEvent) -> i32 {
    event.pointer_id()
}

#[cfg(feature = "wasm")]
fn mark_pointer_active(id: i32) {
    let mut ids = ACTIVE_POINTER_IDS.lock().unwrap();
    if !ids.contains(&id) {
        ids.push(id);
    }
}

#[cfg(feature = "wasm")]
fn clear_pointer_active(id: i32) {
    let mut ids = ACTIVE_POINTER_IDS.lock().unwrap();
    ids.retain(|active| *active != id);
}

#[cfg(feature = "wasm")]
fn pointer_active(id: i32) -> bool {
    ACTIVE_POINTER_IDS.lock().unwrap().contains(&id)
}

#[cfg(feature = "wasm")]
fn pointer_event_targets_canvas(
    canvas: &web_sys::HtmlCanvasElement,
    event: &web_sys::PointerEvent,
) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return false;
    };
    let canvas_node: &web_sys::Node = canvas.unchecked_ref();
    element.is_same_node(Some(canvas_node))
}

#[cfg(feature = "wasm")]
fn install_canvas_input_style(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
    let style = canvas.get_attribute("style").unwrap_or_default();
    let lowered = style.to_ascii_lowercase();
    let mut additions = Vec::new();
    if !lowered.contains("touch-action") {
        additions.push("touch-action:none");
    }
    if !lowered.contains("user-select") {
        additions.push("user-select:none");
    }
    if !lowered.contains("-webkit-user-select") {
        additions.push("-webkit-user-select:none");
    }
    if additions.is_empty() {
        return Ok(());
    }
    let mut next = style;
    if !next.trim().is_empty() && !next.trim_end().ends_with(';') {
        next.push(';');
    }
    next.push_str(&additions.join(";"));
    next.push(';');
    canvas
        .set_attribute("style", &next)
        .map_err(|_| "voplay: failed to set canvas input style".to_string())
}

#[cfg(feature = "wasm")]
fn key_event_targets_text_input(event: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return false;
    };
    let tag = element.tag_name();
    tag.eq_ignore_ascii_case("INPUT")
        || tag.eq_ignore_ascii_case("TEXTAREA")
        || tag.eq_ignore_ascii_case("SELECT")
        || element
            .get_attribute("contenteditable")
            .is_some_and(|value| value.is_empty() || !value.eq_ignore_ascii_case("false"))
}

#[cfg(feature = "wasm")]
fn canvas_has_keyboard_focus(
    document: &web_sys::Document,
    canvas: &web_sys::HtmlCanvasElement,
    event: &web_sys::KeyboardEvent,
) -> bool {
    let canvas_node: &web_sys::Node = canvas.unchecked_ref();
    if document
        .active_element()
        .is_some_and(|active| active.is_same_node(Some(canvas_node)))
    {
        return true;
    }
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|target| target.is_same_node(Some(canvas_node)))
}

#[cfg(feature = "wasm")]
pub fn install_wasm_input_handlers(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
    let already_installed = WASM_INPUT_HANDLERS.with(|handlers| {
        let handlers = handlers.borrow();
        handlers
            .as_ref()
            .is_some_and(|existing| existing.canvas.is_same_node(Some(canvas)))
    });
    if already_installed {
        return Ok(());
    }

    reset_wasm_input_handlers();

    canvas
        .set_attribute("tabindex", "0")
        .map_err(|_| "voplay: failed to make canvas focusable".to_string())?;
    install_canvas_input_style(canvas)?;

    let window = web_sys::window()
        .ok_or_else(|| "voplay: no global window for input handlers".to_string())?;
    let document = window
        .document()
        .ok_or_else(|| "voplay: no document for input handlers".to_string())?;

    let key_canvas = canvas.clone();
    let key_document = document.clone();
    let key_down = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
        let key = event.key();
        if key_event_targets_text_input(&event) {
            return;
        }
        if !canvas_has_keyboard_focus(&key_document, &key_canvas, &event) {
            return;
        }
        if matches!(
            key.as_str(),
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | " " | "PageUp" | "PageDown"
        ) {
            event.prevent_default();
        }
        if !key.is_empty() {
            push_key_event(true, &key);
        }
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback_and_bool(
            "keydown",
            key_down.as_ref().unchecked_ref(),
            true,
        )
        .map_err(|_| "voplay: failed to register keydown listener".to_string())?;

    let key_canvas = canvas.clone();
    let key_document = document.clone();
    let key_up = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
        let key = event.key();
        if key_event_targets_text_input(&event) {
            return;
        }
        if !canvas_has_keyboard_focus(&key_document, &key_canvas, &event) {
            return;
        }
        if !key.is_empty() {
            push_key_event(false, &key);
        }
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback_and_bool("keyup", key_up.as_ref().unchecked_ref(), true)
        .map_err(|_| "voplay: failed to register keyup listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_down = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        event.prevent_default();
        mark_pointer_active(pointer_id_raw(&event));
        let _ = pointer_canvas.focus();
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(
            INPUT_POINTER_DOWN,
            pointer_id(&event),
            x,
            y,
            pointer_button(&event),
        );
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("pointerdown", pointer_down.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointerdown listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_up = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let id = pointer_id_raw(&event);
        if !pointer_active(id) && !pointer_event_targets_canvas(&pointer_canvas, &event) {
            return;
        }
        event.prevent_default();
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(
            INPUT_POINTER_UP,
            pointer_id(&event),
            x,
            y,
            pointer_button(&event),
        );
        clear_pointer_active(id);
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback("pointerup", pointer_up.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointerup listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_move = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let id = pointer_id_raw(&event);
        if !pointer_active(id) && !pointer_event_targets_canvas(&pointer_canvas, &event) {
            return;
        }
        event.prevent_default();
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(INPUT_POINTER_MOVE, pointer_id(&event), x, y, 0);
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback("pointermove", pointer_move.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointermove listener".to_string())?;

    let pointer_canvas = canvas.clone();
    let pointer_cancel = Closure::wrap(Box::new(move |event: web_sys::PointerEvent| {
        let id = pointer_id_raw(&event);
        if !pointer_active(id) && !pointer_event_targets_canvas(&pointer_canvas, &event) {
            return;
        }
        event.prevent_default();
        let (x, y) = pointer_xy(&pointer_canvas, event.client_x(), event.client_y());
        push_pointer_event(
            INPUT_POINTER_UP,
            pointer_id(&event),
            x,
            y,
            pointer_button(&event),
        );
        clear_pointer_active(id);
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback("pointercancel", pointer_cancel.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register pointercancel listener".to_string())?;

    let wheel = Closure::wrap(Box::new(move |event: web_sys::WheelEvent| {
        event.prevent_default();
        push_scroll_event(event.delta_x(), event.delta_y());
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())
        .map_err(|_| "voplay: failed to register wheel listener".to_string())?;
    let _ = canvas.focus();

    WASM_INPUT_HANDLERS.with(|handlers| {
        *handlers.borrow_mut() = Some(WasmInputHandlers {
            canvas: canvas.clone(),
            document,
            key_down,
            key_up,
            pointer_down,
            pointer_up,
            pointer_move,
            pointer_cancel,
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

        let _ = handlers
            .document
            .remove_event_listener_with_callback_and_bool(
                "keydown",
                handlers.key_down.as_ref().unchecked_ref(),
                true,
            );
        let _ = handlers
            .document
            .remove_event_listener_with_callback_and_bool(
                "keyup",
                handlers.key_up.as_ref().unchecked_ref(),
                true,
            );
        let _ = handlers.canvas.remove_event_listener_with_callback(
            "pointerdown",
            handlers.pointer_down.as_ref().unchecked_ref(),
        );
        let _ = handlers.document.remove_event_listener_with_callback(
            "pointerup",
            handlers.pointer_up.as_ref().unchecked_ref(),
        );
        let _ = handlers.document.remove_event_listener_with_callback(
            "pointermove",
            handlers.pointer_move.as_ref().unchecked_ref(),
        );
        let _ = handlers.document.remove_event_listener_with_callback(
            "pointercancel",
            handlers.pointer_cancel.as_ref().unchecked_ref(),
        );
        let _ = handlers
            .canvas
            .remove_event_listener_with_callback("wheel", handlers.wheel.as_ref().unchecked_ref());
    });
    INPUT_BUFFER.lock().unwrap().clear();
    ACTIVE_POINTER_IDS.lock().unwrap().clear();
}

/// Convenience constants for pointer events (re-exported for platform modules).
pub const POINTER_DOWN: u8 = INPUT_POINTER_DOWN;
pub const POINTER_UP: u8 = INPUT_POINTER_UP;
pub const POINTER_MOVE: u8 = INPUT_POINTER_MOVE;
#[allow(dead_code)]
pub const SCROLL: u8 = INPUT_SCROLL;
