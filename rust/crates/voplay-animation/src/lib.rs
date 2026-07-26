pub use voplay_runtime::animation::*;
pub use voplay_runtime::animation_gpu::*;
pub use voplay_runtime::animation_presentation::*;
mod gltf_provider;
pub use gltf_provider::*;

pub const FEATURE_ABI: &str = "voplay.animation/1";

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    FEATURE_ABI.as_ptr() as usize
}
