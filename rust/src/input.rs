//! Input event buffering for voplay.
//! Rust/JS side registers keyboard + pointer listeners, buffers events,
//! and returns packed binary via pollInput extern.

use std::sync::Mutex;

/// Input event kinds (must match input.vo constants).
const INPUT_KEY_DOWN: u8 = 0x01;
const INPUT_KEY_UP: u8 = 0x02;
const INPUT_POINTER_DOWN: u8 = 0x03;
const INPUT_POINTER_UP: u8 = 0x04;
const INPUT_POINTER_MOVE: u8 = 0x05;

/// Global input buffer — events are appended by JS/native listeners,
/// drained by pollInput each frame.
static INPUT_BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Append a key event to the buffer.
pub fn push_key_event(down: bool, key_name: &str) {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    buf.push(if down { INPUT_KEY_DOWN } else { INPUT_KEY_UP });
    // keyCode: u16 (0 for now — name-based lookup)
    buf.extend_from_slice(&0u16.to_le_bytes());
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

/// Drain the input buffer, returning all buffered events.
pub fn drain_input() -> Vec<u8> {
    let mut buf = INPUT_BUFFER.lock().unwrap();
    std::mem::take(&mut *buf)
}

/// Convenience constants for pointer events (re-exported for platform modules).
pub const POINTER_DOWN: u8 = INPUT_POINTER_DOWN;
pub const POINTER_UP: u8 = INPUT_POINTER_UP;
pub const POINTER_MOVE: u8 = INPUT_POINTER_MOVE;
