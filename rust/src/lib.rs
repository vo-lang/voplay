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
mod draw_protocol;
#[cfg(any(feature = "native", feature = "wasm"))]
mod externs;
#[cfg(any(feature = "native", feature = "wasm"))]
mod file_io;
#[cfg(any(feature = "native", feature = "wasm"))]
mod font_manager;
#[cfg(feature = "native")]
mod host_api;
#[cfg(any(feature = "native", feature = "wasm"))]
mod impostor_baker;
#[cfg(any(feature = "native", feature = "wasm"))]
mod input;
#[cfg(any(feature = "native", feature = "wasm"))]
mod material;
mod math3d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod model_loader;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics3d;
mod physics_command;
#[cfg(any(feature = "native", feature = "wasm"))]
mod physics_registry;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline2d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline3d;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline3d_batches;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline3d_material;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_depth;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_post;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_shadow;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_skybox;
#[cfg(any(feature = "native", feature = "wasm"))]
mod pipeline_sprite;
#[cfg(any(feature = "native", feature = "wasm"))]
mod primitive_pipeline;
#[cfg(any(feature = "native", feature = "wasm"))]
mod primitive_scene;
#[cfg(any(feature = "native", feature = "wasm"))]
mod primitives;
#[cfg(any(feature = "native", feature = "wasm"))]
mod render_world;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_frame;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_frame_resources;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_perf;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_runtime;
#[cfg(any(feature = "native", feature = "wasm"))]
mod renderer_targets;
#[cfg(any(feature = "native", feature = "wasm"))]
mod stream;
#[cfg(any(feature = "native", feature = "wasm"))]
mod terrain;
#[cfg(any(feature = "native", feature = "wasm"))]
mod texture;

#[cfg(any(feature = "native", feature = "wasm"))]
pub use renderer::Renderer;
#[cfg(any(feature = "native", feature = "wasm"))]
pub use renderer_runtime::EngineRuntime;

#[cfg(feature = "native")]
pub use host_api::{
    vo_voplay_push_key_event, vo_voplay_push_pointer_down, vo_voplay_push_pointer_move,
    vo_voplay_push_pointer_up, vo_voplay_push_scroll_event, vo_voplay_set_host_api,
};

#[cfg(feature = "wasm-island")]
mod island_bindgen;
#[cfg(test)]
mod island_bindgen_contract;

// The browser island is a complete extension artifact, so it owns the single
// protocol identity consumed from WebAssembly.Instance.exports by Studio.
// Keep this at the artifact root: dependency extension crates must not emit a
// second protocol symbol into the same module.
#[cfg(all(feature = "wasm-island", target_arch = "wasm32"))]
vo_ext::export_wasm_extension_protocol!();

#[cfg(all(test, feature = "wasm-island", target_arch = "wasm32"))]
mod wasm_protocol_contract {
    #[test]
    fn browser_artifact_exports_protocol_v3() {
        assert_eq!(
            super::vo_ext_protocol_version(),
            vo_ext::WASM_EXTENSION_PROTOCOL_VERSION,
        );
        assert_eq!(super::vo_ext_protocol_version(), 3);
    }
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use vo_runtime::bytecode::ExternDef;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use vo_runtime::ffi::ExternRegistry;

/// Register this extension's statically linked browser providers.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub fn register_externs(registry: &mut ExternRegistry, externs: &[ExternDef]) {
    externs::vo_ext_register(registry, externs);
}

/// Force link the process-local native catalogs for voplay and vogui.
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub fn ensure_linked() {
    let _ = std::hint::black_box(externs::vo_ext_get_entries());
    vo_vogui::ensure_linked();
}
