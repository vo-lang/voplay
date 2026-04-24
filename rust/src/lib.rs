//! VoPlay game engine — Rust renderer, physics, and input.
//!
//! # Features
//! - `native` (default): dynamic library for dlopen
//! - `wasm`: compiled into the playground/studio WASM binary
//! - `wasm-standalone`: pure C-ABI cdylib for dynamic WASM loading

#[cfg(any(feature = "native", feature = "wasm"))]
mod animation;
#[cfg(any(feature = "native", feature = "wasm"))]
mod draw_list;
#[cfg(any(feature = "native", feature = "wasm"))]
mod externs;
#[cfg(any(feature = "native", feature = "wasm"))]
mod file_io;
#[cfg(any(feature = "native", feature = "wasm"))]
mod font_manager;
#[cfg(feature = "native")]
mod host_api;
#[cfg(any(feature = "native", feature = "wasm"))]
mod input;
#[allow(dead_code)]
mod math3d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod model_loader;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics3d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics_registry;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline2d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline3d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_shadow;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_skybox;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_sprite;
#[cfg(any(feature = "native", feature = "wasm"))]
mod primitives;
#[cfg(any(feature = "native", feature = "wasm"))]
mod render_world;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_runtime;
#[cfg(any(feature = "native", feature = "wasm"))]
mod stream;
#[cfg(any(feature = "native", feature = "wasm"))]
mod terrain;
#[cfg(any(feature = "native", feature = "wasm"))]
mod texture;

#[cfg(any(feature = "native", feature = "wasm"))]
pub use renderer::Renderer;

#[cfg(feature = "native")]
pub use host_api::{
    vo_voplay_push_key_event, vo_voplay_push_pointer_down, vo_voplay_push_pointer_move,
    vo_voplay_push_pointer_up, vo_voplay_push_scroll_event, vo_voplay_set_host_api,
};

#[cfg(feature = "native")]
vo_ext::export_extensions!();

#[cfg(feature = "wasm-island")]
mod island_bindgen;

#[cfg(any(feature = "native", feature = "wasm"))]
use vo_runtime::bytecode::ExternDef;
#[cfg(any(feature = "native", feature = "wasm"))]
use vo_runtime::ffi::ExternRegistry;

#[cfg(any(feature = "native", feature = "wasm"))]
pub fn register_externs(registry: &mut ExternRegistry, externs: &[ExternDef]) {
    externs::vo_ext_register(registry, externs);
}

#[cfg(any(feature = "native", feature = "wasm"))]
/// Force link this crate's FFI functions (including vogui's).
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_linked() {
    let _ = std::hint::black_box(register_externs as fn(&mut ExternRegistry, &[ExternDef]));
    #[cfg(feature = "native")]
    vo_vogui::ensure_linked();
}
