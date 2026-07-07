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
pub fn set_renderer(renderer: crate::renderer::Renderer) -> Result<(), String> {
    crate::renderer_runtime::set_renderer(renderer)
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

pub(crate) fn last_renderer_perf_packet() -> Result<Vec<u8>, String> {
    crate::renderer_runtime::last_renderer_perf_packet()
}

pub(crate) fn set_renderer_perf_stats_enabled(enabled: bool) -> Result<(), String> {
    crate::renderer_runtime::set_renderer_perf_stats_enabled(enabled)
}

#[cfg(not(target_arch = "wasm32"))]
vo_ext::export_extensions!();

#[cfg(target_arch = "wasm32")]
vo_ext::export_extensions!(
    render::__EXT_voplay_initSurface,
    render::__EXT_voplay_submitFrame,
    render::__EXT_voplay_setRendererPerfStatsEnabled,
    render::__EXT_voplay_lastRendererPerfPacket,
    render::__EXT_voplay_lastWebGpuPerfPacket,
    render::__EXT_voplay_pollInput,
    render::__EXT_voplay_waitDisplayPulse,
    render::__EXT_voplay_loadTexture,
    render::__EXT_voplay_loadTextureBytes,
    render::__EXT_voplay_loadTextureLinear,
    render::__EXT_voplay_loadTextureBytesLinear,
    render::__EXT_voplay_loadTextureRGBA,
    render::__EXT_voplay_loadTextureRGBALinear,
    render::__EXT_voplay_texturePixelsBytes,
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
    resource::__EXT_voplay_modelGeometryBytes,
    resource::__EXT_voplay_scene3d_bakeImpostorAtlasBytes,
    resource::__EXT_voplay_scene3d_loadLevel,
    resource::__EXT_voplay_scene3d_createTerrain,
    resource::__EXT_voplay_scene3d_createTerrainSplat,
    resource::__EXT_voplay_scene3d_createTerrainSplatModel,
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
    resource::__EXT_voplay_createRawMesh,
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
    physics3d::__EXT_voplay_scene3d_physicsCreateRaycastVehicle,
    physics3d::__EXT_voplay_scene3d_physicsDestroyRaycastVehicle,
    physics3d::__EXT_voplay_scene3d_physicsAddRaycastVehicleWheel,
    physics3d::__EXT_voplay_scene3d_physicsSetRaycastVehicleWheelControl,
    physics3d::__EXT_voplay_scene3d_physicsApplyRaycastVehicleForces,
    physics3d::__EXT_voplay_scene3d_physicsSetBodyPose,
    physics3d::__EXT_voplay_scene3d_physicsSetBodyMotion,
    physics3d::__EXT_voplay_scene3d_physicsSetBodySleepState,
    physics3d::__EXT_voplay_scene3d_physicsRaycastVehicleState,
    physics3d::__EXT_voplay_scene3d_physicsStep,
    physics3d::__EXT_voplay_scene3d_physicsSetGravity,
    physics3d::__EXT_voplay_scene3d_physicsContacts,
    physics3d::__EXT_voplay_scene3d_physicsRayCast,
    physics3d::__EXT_voplay_scene3d_physicsQueryAABB,
);

pub fn vo_ext_register(registry: &mut ExternRegistry, externs: &[ExternDef]) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = registry.register_from_linkme(externs);
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn native_extension_table_names_are_unique() {
        let table = super::vo_ext_get_entries();
        let entries = if table.entry_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(table.entries, table.entry_count as usize) }
        };
        let mut seen = HashSet::with_capacity(entries.len());
        for entry in entries {
            let name = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    entry.name_ptr,
                    entry.name_len as usize,
                ))
            };
            assert!(
                seen.insert(name.to_string()),
                "duplicate extern entry {name}"
            );
        }
    }
}
