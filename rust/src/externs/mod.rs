//! Vo extern implementations for voplay.
//!
//! Split into sub-modules by domain:
//!   render   — surface init, frame submit, input poll, runtime query, native loop, texture
//!   physics2d — scene2d physics externs
//!   physics3d — scene3d physics externs
//!   audio    — audio load/play/control externs
//!   resource — font and model load/free externs

mod render;
mod physics2d;
mod physics3d_externs;
mod audio;
mod resource;

use std::sync::{Mutex, OnceLock};
use crate::renderer::Renderer;

/// Global renderer instance for the WASM/web path.
/// Native path uses NativeApp.renderer instead.
pub(crate) static RENDERER: OnceLock<Mutex<Renderer>> = OnceLock::new();

/// Set the renderer from pre-initialized wgpu parts (used by host/studio integration).
#[allow(dead_code)]
pub fn set_renderer(renderer: Renderer) {
    let _ = RENDERER.set(Mutex::new(renderer));
}

/// Access the renderer, dispatching to native APP or global RENDERER.
pub(crate) fn with_renderer<R>(f: impl FnOnce(&mut Renderer) -> R) -> Result<R, String> {
    #[cfg(feature = "native")]
    {
        crate::native::with_renderer(f)
    }
    #[cfg(not(feature = "native"))]
    {
        match RENDERER.get() {
            Some(mutex) => {
                let mut renderer = mutex.lock().unwrap();
                Ok(f(&mut renderer))
            }
            None => Err("voplay: renderer not initialized".to_string()),
        }
    }
}
