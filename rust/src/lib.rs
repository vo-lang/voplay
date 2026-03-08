//! VoPlay game engine — Rust renderer, physics, and input.
//!
//! # Features
//! - `native` (default): dynamic library for dlopen
//! - `wasm`: compiled into the playground/studio WASM binary
//! - `wasm-standalone`: pure C-ABI cdylib for dynamic WASM loading

#[cfg(any(feature = "native", feature = "wasm"))]
mod externs;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer;
#[cfg(any(feature = "native", feature = "wasm"))]
mod stream;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline2d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod font_manager;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics;
#[cfg(any(feature = "native", feature = "wasm"))]
mod audio;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_sprite;
#[cfg(any(feature = "native", feature = "wasm"))]
mod texture;
#[cfg(any(feature = "native", feature = "wasm"))]
mod input;
#[cfg(feature = "native")]
pub mod native;

#[cfg(any(feature = "native", feature = "wasm"))]
pub use renderer::Renderer;

#[cfg(feature = "native")]
vo_ext::export_extensions!();

#[cfg(any(feature = "native", feature = "wasm"))]
/// Force link this crate's FFI functions (including vogui's).
pub fn ensure_linked() {
    // Link voplay externs
    extern "C" {
        fn vo_ext_get_entries() -> vo_ext::ExtensionTable;
    }
    let _ = std::hint::black_box(unsafe { vo_ext_get_entries() });
    // Link vogui externs (voplay imports vogui, so VM needs them resolved)
    #[cfg(feature = "native")]
    vogui::ensure_linked();
}
