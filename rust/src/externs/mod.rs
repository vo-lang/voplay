//! Vo extern implementations for voplay.
//!
//! Split into sub-modules by domain:
//!   render   — surface init, frame submit, input poll, renderer query, texture
//!   physics2d — scene2d physics externs
//!   physics3d — scene3d physics externs
//!   animation — scene3d skeletal animation externs
//!   audio    — audio load/play/control externs
//!   resource — font and model load/free externs
//!   util     — common decode and result helpers

pub(crate) mod animation;
pub(crate) mod audio;
pub(crate) mod physics2d;
pub(crate) mod physics3d;
pub(crate) mod render;
pub(crate) mod resource;
pub(crate) mod util;

use vo_runtime::bytecode::ExternDef;
use vo_runtime::ffi::ExternRegistry;

/// Set the renderer from pre-initialized wgpu parts (used by host/studio integration).
#[allow(dead_code)]
#[cfg(not(target_arch = "wasm32"))]
pub fn set_renderer(renderer: crate::renderer::Renderer) {
    crate::renderer_runtime::set_renderer(renderer);
}

/// Access the renderer, dispatching to hosted runtime state or native APP.
pub(crate) fn with_renderer<R>(
    f: impl FnOnce(&mut crate::renderer::Renderer) -> R,
) -> Result<R, String> {
    crate::renderer_runtime::with_renderer(f)
}

/// Check if the renderer is ready.
pub(crate) fn renderer_ready() -> bool {
    crate::renderer_runtime::renderer_ready()
}

pub(crate) fn renderer_ready_result() -> Result<bool, String> {
    crate::renderer_runtime::renderer_ready_result()
}

/// Submit a frame to the active renderer runtime.
pub(crate) fn submit_renderer_frame(data: &[u8]) -> Result<(), String> {
    crate::renderer_runtime::submit_renderer_frame(data)
}

#[cfg(target_arch = "wasm32")]
const VO_EXT_ENTRIES: &[vo_runtime::ffi::StdlibEntry] = &[
    render::__EXT_voplay_initSurface,
    render::__EXT_voplay_submitFrame,
    render::__EXT_voplay_pollInput,
    render::__EXT_voplay_loadTexture,
    render::__EXT_voplay_loadTextureBytes,
    render::__EXT_voplay_freeTexture,
    render::__EXT_voplay_loadCubemap,
    render::__EXT_voplay_loadCubemapBytes,
    render::__EXT_voplay_freeCubemap,
    render::__EXT_voplay_isRendererReady,
    resource::__EXT_voplay_loadFont,
    resource::__EXT_voplay_loadFontBytes,
    resource::__EXT_voplay_freeFont,
    resource::__EXT_voplay_measureText,
    resource::__EXT_voplay_loadModel,
    resource::__EXT_voplay_loadModelBytes,
    resource::__EXT_voplay_freeModel,
    resource::__EXT_voplay_modelBounds,
    resource::__EXT_voplay_modelMeshDataBytes,
    resource::__EXT_voplay_scene3d_loadLevel,
    resource::__EXT_voplay_scene3d_createTerrain,
    resource::__EXT_voplay_scene3d_createTerrainSplat,
    resource::__EXT_voplay_scene3d_createTerrainBytes,
    resource::__EXT_voplay_scene3d_createTerrainBytesSplat,
    resource::__EXT_voplay_scene3d_terrainHeightAt,
    resource::__EXT_voplay_createPlaneMesh,
    resource::__EXT_voplay_createCubeMesh,
    resource::__EXT_voplay_createRoundedBoxMesh,
    resource::__EXT_voplay_createSphereMesh,
    resource::__EXT_voplay_createCylinderMesh,
    resource::__EXT_voplay_createConeMesh,
    resource::__EXT_voplay_createWedgeMesh,
    resource::__EXT_voplay_createCapsuleMesh,
    audio::__EXT_voplay_audioLoadFile,
    animation::__EXT_voplay_animationInit,
    animation::__EXT_voplay_animationDestroy,
    animation::__EXT_voplay_animationPlay,
    animation::__EXT_voplay_animationStop,
    animation::__EXT_voplay_animationCrossfade,
    animation::__EXT_voplay_animationSetSpeed,
    animation::__EXT_voplay_animationRemoveTarget,
    animation::__EXT_voplay_animationTick,
    animation::__EXT_voplay_animationProgress,
    animation::__EXT_voplay_animationModelInfo,
    physics2d::__EXT_voplay_scene2d_physicsInit,
    physics2d::__EXT_voplay_scene2d_physicsDestroy,
    physics2d::__EXT_voplay_scene2d_physicsSpawnBody,
    physics2d::__EXT_voplay_scene2d_physicsDestroyBody,
    physics2d::__EXT_voplay_scene2d_physicsStep,
    physics2d::__EXT_voplay_scene2d_physicsSetGravity,
    physics2d::__EXT_voplay_scene2d_physicsContacts,
    physics2d::__EXT_voplay_scene2d_physicsRayCast,
    physics2d::__EXT_voplay_scene2d_physicsQueryRect,
    physics3d::__EXT_voplay_scene3d_physicsInit,
    physics3d::__EXT_voplay_scene3d_physicsDestroy,
    physics3d::__EXT_voplay_scene3d_physicsSpawnBody,
    physics3d::__EXT_voplay_scene3d_physicsSpawnTrimeshBody,
    physics3d::__EXT_voplay_scene3d_physicsSpawnTrimeshBodyData,
    physics3d::__EXT_voplay_scene3d_physicsSpawnHeightfield,
    physics3d::__EXT_voplay_scene3d_physicsDestroyBody,
    physics3d::__EXT_voplay_scene3d_physicsStep,
    physics3d::__EXT_voplay_scene3d_physicsSetGravity,
    physics3d::__EXT_voplay_scene3d_physicsContacts,
    physics3d::__EXT_voplay_scene3d_physicsRayCast,
    physics3d::__EXT_voplay_scene3d_physicsQueryAABB,
];

pub fn vo_ext_register(registry: &mut ExternRegistry, externs: &[ExternDef]) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        registry.register_from_linkme(externs);
    }

    #[cfg(target_arch = "wasm32")]
    {
        fn find_id(externs: &[ExternDef], name: &str) -> Option<u32> {
            externs
                .iter()
                .position(|d| d.name == name)
                .map(|i| i as u32)
        }

        for entry in VO_EXT_ENTRIES {
            if let Some(id) = find_id(externs, entry.name()) {
                entry.register(registry, id);
            }
        }
    }
}
