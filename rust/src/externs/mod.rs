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

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use vo_runtime::bytecode::ExternDef;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use vo_runtime::ffi::ExternRegistry;

/// Access the renderer, dispatching to hosted runtime state or native APP.
pub(crate) fn with_renderer<R>(
    f: impl FnOnce(&mut crate::renderer::Renderer) -> R,
) -> Result<R, String> {
    crate::renderer_runtime::with_renderer(f)
}

/// Check if the renderer is ready.
#[cfg(not(feature = "wasm"))]
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

// A native dylib is validated against the exact owner declared by its vo.mod,
// so its ABI table must contain only voplay entries. The same authoritative
// list is also used for statically registered browser providers.
#[cfg(any(
    all(feature = "native", not(target_arch = "wasm32")),
    all(feature = "wasm", target_arch = "wasm32"),
))]
vo_ext::export_extensions!(
    vo_ext::vo_extension_entry!(render, "voplay", "initSurface"),
    vo_ext::vo_extension_entry!(render, "voplay", "submitFrame"),
    vo_ext::vo_extension_entry!(render, "voplay", "setRendererPerfStatsEnabled"),
    vo_ext::vo_extension_entry!(render, "voplay", "lastRendererPerfPacket"),
    vo_ext::vo_extension_entry!(render, "voplay", "lastWebGpuPerfPacket"),
    vo_ext::vo_extension_entry!(render, "voplay", "pollInput"),
    vo_ext::vo_extension_entry!(render, "voplay", "waitDisplayPulse"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTexture"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTextureBytes"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTextureLinear"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTextureBytesLinear"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTextureRGBA"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadTextureRGBALinear"),
    vo_ext::vo_extension_entry!(render, "voplay", "texturePixelsBytes"),
    vo_ext::vo_extension_entry!(render, "voplay", "freeTexture"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadCubemap"),
    vo_ext::vo_extension_entry!(render, "voplay", "loadCubemapBytes"),
    vo_ext::vo_extension_entry!(render, "voplay", "freeCubemap"),
    vo_ext::vo_extension_entry!(render, "voplay", "isRendererReady"),
    vo_ext::vo_extension_entry!(resource, "voplay", "loadFont"),
    vo_ext::vo_extension_entry!(resource, "voplay", "loadFontBytes"),
    vo_ext::vo_extension_entry!(resource, "voplay", "freeFont"),
    vo_ext::vo_extension_entry!(resource, "voplay", "measureText"),
    vo_ext::vo_extension_entry!(resource, "voplay", "loadModel"),
    vo_ext::vo_extension_entry!(resource, "voplay", "loadModelBytes"),
    vo_ext::vo_extension_entry!(resource, "voplay", "freeModel"),
    vo_ext::vo_extension_entry!(resource, "voplay", "modelBounds"),
    vo_ext::vo_extension_entry!(resource, "voplay", "modelGeometryBytes"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "bakeImpostorAtlasBytes"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "loadLevel"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "createTerrain"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "createTerrainSplat"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "createTerrainSplatModel"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "createTerrainBytes"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "createTerrainBytesSplat"),
    vo_ext::vo_extension_entry!(resource, "voplay/scene3d", "terrainHeightAt"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createPlaneMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createCubeMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createRoundedBoxMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createSphereMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createCylinderMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createConeMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createWedgeMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createCapsuleMesh"),
    vo_ext::vo_extension_entry!(resource, "voplay", "createRawMesh"),
    vo_ext::vo_extension_entry!(audio, "voplay", "audioLoadFile"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationInit"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationDestroy"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationPlay"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationStop"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationCrossfade"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationSetSpeed"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationRemoveTarget"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationTick"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationProgress"),
    vo_ext::vo_extension_entry!(animation, "voplay", "animationModelInfo"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsInit"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsDestroy"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsSpawnBody"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsDestroyBody"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsStep"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsSetGravity"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsContacts"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsRayCast"),
    vo_ext::vo_extension_entry!(physics2d, "voplay/scene2d", "physicsQueryRect"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsInit"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsDestroy"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSpawnBody"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSpawnTrimeshBody"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSpawnTrimeshBodyData"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSpawnHeightfield"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsDestroyBody"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsCreateRaycastVehicle"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsDestroyRaycastVehicle"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsAddRaycastVehicleWheel"),
    vo_ext::vo_extension_entry!(
        physics3d,
        "voplay/scene3d",
        "physicsSetRaycastVehicleWheelControl"
    ),
    vo_ext::vo_extension_entry!(
        physics3d,
        "voplay/scene3d",
        "physicsApplyRaycastVehicleForces"
    ),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSetBodyPose"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSetBodyMotion"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSetBodySleepState"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsRaycastVehicleState"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsRaycastVehicleStates"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsStep"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsSetGravity"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsContacts"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsRayCast"),
    vo_ext::vo_extension_entry!(physics3d, "voplay/scene3d", "physicsQueryAABB"),
);

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub fn vo_ext_register(registry: &mut ExternRegistry, externs: &[ExternDef]) {
    fn find_id(externs: &[ExternDef], name: &str) -> Option<u32> {
        externs
            .iter()
            .position(|definition| definition.name == name)
            .map(|index| index as u32)
    }

    for entry in VO_EXT_ENTRIES {
        if let Some(id) = find_id(externs, entry.name()) {
            entry.register(registry, id);
        }
    }
}

#[cfg(all(test, feature = "native", not(target_arch = "wasm32")))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use vo_runtime::bytecode::ExternDef;
    use vo_runtime::ffi::{ExternRegistry, EXTERN_MODULE_OWNER_TABLE, EXTERN_TABLE};

    fn utf8_field(pointer: *const u8, length: u32) -> String {
        assert!(!pointer.is_null(), "extension table field must not be null");
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length as usize) };
        std::str::from_utf8(bytes)
            .expect("extension table field must be valid UTF-8")
            .to_owned()
    }

    #[test]
    fn native_extension_table_is_exactly_voplay_owned() {
        let table = super::vo_ext_get_entries();
        let entries = if table.entry_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(table.entries, table.entry_count as usize) }
        };
        let mut seen = BTreeSet::new();
        for entry in entries {
            let name = utf8_field(entry.name_ptr, entry.name_len);
            let owner = utf8_field(entry.module_owner_ptr, entry.module_owner_len);
            assert_eq!(
                owner, "github.com/vo-lang/voplay",
                "native dylib entry {name} escaped the artifact owner boundary",
            );
            assert!(seen.insert(name.clone()), "duplicate extern entry {name}");
        }
    }

    #[test]
    fn native_catalog_contains_voplay_and_embedded_vogui() {
        crate::ensure_linked();
        vo_runtime::ffi::validate_linkme_extern_table()
            .expect("the complete statically linked extension catalog must be valid");

        let expected_owners = BTreeSet::from([
            "github.com/vo-lang/vogui".to_owned(),
            "github.com/vo-lang/voplay".to_owned(),
        ]);
        let mut owner_declaration_counts = BTreeMap::<String, usize>::new();
        for entry in EXTERN_MODULE_OWNER_TABLE {
            *owner_declaration_counts
                .entry(utf8_field(entry.module_owner_ptr, entry.module_owner_len))
                .or_default() += 1;
        }
        assert_eq!(
            owner_declaration_counts
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            expected_owners,
            "native catalog owner declarations diverged",
        );
        assert!(
            owner_declaration_counts.values().all(|count| *count == 1),
            "each native module owner must be declared exactly once: {owner_declaration_counts:?}",
        );

        let exported_table = super::vo_ext_get_entries();
        let exported_entries = if exported_table.entry_count == 0 {
            &[][..]
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    exported_table.entries,
                    exported_table.entry_count as usize,
                )
            }
        };
        let exported_entries = exported_entries
            .iter()
            .map(|entry| {
                (
                    utf8_field(entry.module_owner_ptr, entry.module_owner_len),
                    utf8_field(entry.name_ptr, entry.name_len),
                )
            })
            .collect::<BTreeSet<_>>();
        let linked_entries = EXTERN_TABLE
            .iter()
            .map(|entry| {
                (
                    utf8_field(entry.module_owner_ptr, entry.module_owner_len),
                    utf8_field(entry.name_ptr, entry.name_len),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            exported_entries,
            linked_entries
                .iter()
                .filter(|(owner, _)| owner == "github.com/vo-lang/voplay")
                .cloned()
                .collect::<BTreeSet<_>>(),
            "the dylib table must export every voplay entry and no dependency entries",
        );
        assert_eq!(
            linked_entries
                .iter()
                .map(|(owner, _)| owner.clone())
                .collect::<BTreeSet<_>>(),
            expected_owners,
            "every declared native owner must contribute extern entries",
        );

        let definitions = EXTERN_TABLE
            .iter()
            .map(|entry| {
                ExternDef::call_site_variadic(
                    utf8_field(entry.name_ptr, entry.name_len),
                    0,
                    entry
                        .effects()
                        .expect("generated native extern effects must be valid"),
                    Vec::new(),
                )
            })
            .collect::<Vec<_>>();
        let mut registry = ExternRegistry::new();
        registry
            .register_from_extension_catalogs(None, &definitions)
            .expect("the combined voplay and vogui catalog must register atomically");

        for (owner, name) in linked_entries {
            let registered = registry
                .registered_by_name(&name)
                .unwrap_or_else(|| panic!("native catalog entry {name} was not registered"));
            assert_eq!(
                registered.provider_module_owner(),
                Some(owner.as_str()),
                "wrong provider owner for {name}",
            );
        }
    }
}
