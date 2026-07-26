mod features;
pub use features::*;
mod extended_features;
pub use extended_features::*;
mod visibility;
pub use visibility::*;
mod shadow;
pub use shadow::*;
mod wire;
pub use wire::*;
mod terrain;
pub use terrain::*;
mod heightmap_asset;
pub use heightmap_asset::*;
mod decal;
pub use decal::*;
mod gltf_pose;
pub use gltf_pose::*;
mod encoder;
pub use encoder::*;
mod scene_frame;
pub use scene_frame::*;
mod compiled_features;
pub use compiled_features::*;
mod mesh_asset;
pub use mesh_asset::*;
mod skin_palette_asset;
pub use skin_palette_asset::*;

#[cfg(feature = "native-wgpu")]
mod wgpu_renderer;
#[cfg(feature = "native-wgpu")]
pub use wgpu_renderer::*;
#[cfg(feature = "native-wgpu")]
mod asset_residency;
#[cfg(feature = "native-wgpu")]
mod data_feature_wgpu;
#[cfg(feature = "native-wgpu")]
pub use asset_residency::*;
#[cfg(feature = "native-wgpu")]
mod native_device;
#[cfg(feature = "native-wgpu")]
pub use native_device::*;
#[cfg(feature = "native-wgpu")]
mod frame_trace_wire;
#[cfg(feature = "native-wgpu")]
pub use frame_trace_wire::*;
#[cfg(feature = "native-wgpu")]
mod gltf_upload;
#[cfg(feature = "native-wgpu")]
pub use gltf_upload::*;
#[cfg(feature = "native-wgpu")]
mod animation_backend;
#[cfg(feature = "native-wgpu")]
pub use animation_backend::*;

pub use voplay_runtime::animation_gpu::{
    encode_commands, verify_encoding, AnimationGpuAck, AnimationGpuAdapter, AnimationGpuBackend,
    AnimationGpuBackendFailure, AnimationGpuBatch, AnimationGpuCommand, AnimationGpuConfig,
    AnimationGpuError, AnimationGpuState,
};
pub use voplay_runtime::render_graph::{
    compile, CompiledRenderGraph, GraphNodeId, GraphNodeSpec, GraphQueue, GraphResourceDesc,
    GraphResourceId, RenderGraphConfig, RenderGraphError, ResourceLifetime, ResourceRead,
    ResourceUsage, ResourceWrite, TextureFormat,
};
pub use voplay_runtime::render_world::{
    Aabb3, RenderComponent, RenderComponentKind, RetainedRenderObject, TransformSample,
};

pub const FEATURE_ABI: &str = "voplay.render3d/1";

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    FEATURE_ABI.as_ptr() as usize
}
