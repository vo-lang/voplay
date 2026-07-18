use super::*;
use crate::renderer::{TerrainCreateOptions, TerrainSplatMaterialOptions, TerrainSplatOptions};

// ── Resource externs ──────────────────────────────────────────────────────────

/// loadFont(path string) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadFont")]
pub fn load_font(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = match crate::externs::with_renderer(|r| r.load_font(&path)) {
        Ok(result) => result,
        Err(_) => res::with_headless_font_manager_pub(|fonts| fonts.load_file(&path))
            .and_then(|result| result),
    };
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadFontBytes(data []byte) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadFontBytes")]
pub fn load_font_bytes(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = match crate::externs::with_renderer(|r| r.load_font_bytes(data.clone())) {
        Ok(result) => result,
        Err(_) => res::with_headless_font_manager_pub(|fonts| fonts.load_bytes(data))
            .and_then(|result| result),
    };
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeFont(id uint32)
#[vo_ext::vo_wasm_bindgen_export("voplay", "freeFont")]
pub fn free_font(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    if crate::externs::with_renderer(|r| r.free_font(id)).is_err() {
        res::with_headless_font_manager_pub(|fonts| fonts.free(id))
            .unwrap_or_else(|msg| panic!("{}", msg));
    }
    Vec::new()
}

/// measureText(fontId uint32, text string, size float) → (float, float)
#[vo_ext::vo_wasm_bindgen_export("voplay", "measureText")]
pub fn measure_text(input: &[u8]) -> Vec<u8> {
    use crate::externs::resource as res;
    let mut pos = DecodePosition::new(input);
    let font_id = in_value(input, &mut pos) as u32;
    let text = in_str(input, &mut pos).to_string();
    let size = in_f64(input, &mut pos) as f32;
    pos.finish();
    let (w, h) = match crate::externs::with_renderer(|r| r.measure_text(font_id, &text, size)) {
        Ok(result) => result,
        Err(_) => {
            res::with_headless_font_manager_pub(|fonts| fonts.measure_text(font_id, &text, size))
                .unwrap_or_else(|msg| panic!("{}", msg))
        }
    };
    let mut out = Vec::new();
    out_value_f64(&mut out, w as f64);
    out_value_f64(&mut out, h as f64);
    out
}

/// loadModel(path string) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadModel")]
pub fn load_model(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_model(&path));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// loadModelBytes(data []byte) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "loadModelBytes")]
pub fn load_model_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_model_bytes(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// createRawMesh(data []byte) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "createRawMesh")]
pub fn create_raw_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.create_raw_mesh(&data));
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// freeModel(id uint32)
#[vo_ext::vo_wasm_bindgen_export("voplay", "freeModel")]
pub fn free_model(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let _ = crate::externs::with_renderer(|r| r.free_model(id));
    Vec::new()
}

/// modelBounds(id uint32) → (float, float, float, float, float, float, bool)
#[vo_ext::vo_wasm_bindgen_export("voplay", "modelBounds")]
pub fn model_bounds(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let mut out = Vec::new();
    match crate::externs::with_renderer(|r| r.model_bounds(id)) {
        Ok(Some((min, max))) => {
            out_value_f64(&mut out, min[0] as f64);
            out_value_f64(&mut out, min[1] as f64);
            out_value_f64(&mut out, min[2] as f64);
            out_value_f64(&mut out, max[0] as f64);
            out_value_f64(&mut out, max[1] as f64);
            out_value_f64(&mut out, max[2] as f64);
            out_value_bool(&mut out, true);
        }
        _ => {
            for _ in 0..6 {
                out_value_f64(&mut out, 0.0);
            }
            out_value_bool(&mut out, false);
        }
    }
    out
}

/// modelGeometryBytes(id uint32) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay", "modelGeometryBytes")]
pub fn model_geometry_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let id = in_value(input, &mut pos) as u32;
    pos.finish();
    let geometry = crate::externs::util::with_renderer_or_panic("modelGeometryBytes", |renderer| {
        renderer.get_model_geometry(id)
    });
    let geometry =
        geometry.unwrap_or_else(|| panic!("modelGeometryBytes: model not found: {}", id));
    let data = crate::externs::resource::encode_model_geometry_bytes(&geometry);
    let mut out = Vec::new();
    out_bytes(&mut out, &data);
    out
}

// ── scene3d resource externs ──────────────────────────────────────────────────

/// scene3d_bakeImpostorAtlasBytes(request []byte) → ([]byte, error)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "bakeImpostorAtlasBytes")]
pub fn scene3d_bake_impostor_atlas_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let request = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let result = crate::impostor_baker::bake_impostor_atlas_bytes(&request);
    let mut out = Vec::new();
    out_bytes_result(&mut out, result);
    out
}

/// scene3d_loadLevel(path string) → ([]byte, error)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "loadLevel")]
pub fn scene3d_load_level(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| r.load_level(&path))
        .map(|nodes| crate::externs::resource::serialize_level_nodes(&nodes));
    let mut out = Vec::new();
    out_bytes_result(&mut out, result);
    out
}

/// scene3d_createTerrain(path, sx, sy, sz, uvScale, texId, normalTexId, mrTexId, normalScale, roughness, metallic) → terrain result
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "createTerrain")]
pub fn scene3d_create_terrain(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let uv_scale = in_f64(input, &mut pos) as f32;
    let texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let metallic_roughness_texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_scale = in_f64(input, &mut pos) as f32;
    let roughness = in_f64(input, &mut pos) as f32;
    let metallic = in_f64(input, &mut pos) as f32;
    pos.finish();
    let result = crate::file_io::read_bytes(&path)
        .map_err(|e| format!("terrain: read {}: {}", path, e))
        .and_then(|data| {
            crate::externs::util::with_renderer_result(|r| {
                r.create_terrain(
                    &data,
                    TerrainCreateOptions {
                        scale: [scale_x, scale_y, scale_z],
                        uv_scale,
                        texture_id,
                        normal_texture_id,
                        metallic_roughness_texture_id,
                        normal_scale,
                        roughness,
                        metallic,
                    },
                )
            })
        });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainSplat(path, sx, sy, sz, controlTexId, layerData) → terrain result
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "createTerrainSplat")]
pub fn scene3d_create_terrain_splat(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let args = decode_terrain_splat_input(input, &mut pos);
    pos.finish();
    let result = args.and_then(
        |(
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            normal_scales,
            terrain_tuning,
        )| {
            crate::file_io::read_bytes(&path)
                .map_err(|e| format!("terrain: read {}: {}", path, e))
                .and_then(|data| {
                    crate::externs::util::with_renderer_result(|r| {
                        r.create_terrain_splat(
                            &data,
                            TerrainSplatOptions {
                                scale: [scale_x, scale_y, scale_z],
                                material: TerrainSplatMaterialOptions {
                                    control_texture_id,
                                    layer_texture_ids,
                                    layer_normal_texture_ids,
                                    layer_metallic_roughness_texture_ids,
                                    uv_scales,
                                    layer_normal_scales: normal_scales,
                                    terrain_tuning,
                                },
                            },
                        )
                    })
                })
        },
    );
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainSplatModel(modelId, controlTexId, layerData) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "createTerrainSplatModel")]
pub fn scene3d_create_terrain_splat_model(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let model_id = in_value(input, &mut pos) as u32;
    let args = decode_terrain_splat_input(input, &mut pos);
    pos.finish();
    let result = args.and_then(
        |(
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            normal_scales,
            terrain_tuning,
        )| {
            crate::externs::util::with_renderer_result(|r| {
                r.create_terrain_splat_model(
                    model_id,
                    TerrainSplatMaterialOptions {
                        control_texture_id,
                        layer_texture_ids,
                        layer_normal_texture_ids,
                        layer_metallic_roughness_texture_ids,
                        uv_scales,
                        layer_normal_scales: normal_scales,
                        terrain_tuning,
                    },
                )
            })
        },
    );
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}

/// scene3d_createTerrainBytes(data []byte, sx, sy, sz, uvScale, texId, normalTexId, mrTexId, normalScale, roughness, metallic) → terrain result
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "createTerrainBytes")]
pub fn scene3d_create_terrain_bytes(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let uv_scale = in_f64(input, &mut pos) as f32;
    let texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let metallic_roughness_texture_id = match in_value(input, &mut pos) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_scale = in_f64(input, &mut pos) as f32;
    let roughness = in_f64(input, &mut pos) as f32;
    let metallic = in_f64(input, &mut pos) as f32;
    pos.finish();
    let result = crate::externs::util::with_renderer_result(|r| {
        r.create_terrain(
            &data,
            TerrainCreateOptions {
                scale: [scale_x, scale_y, scale_z],
                uv_scale,
                texture_id,
                normal_texture_id,
                metallic_roughness_texture_id,
                normal_scale,
                roughness,
                metallic,
            },
        )
    });
    crate::externs::resource::encode_terrain_result_bytes(result)
}

/// scene3d_createTerrainBytesSplat(data, sx, sy, sz, controlTexId, layerData) → terrain result
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "createTerrainBytesSplat")]
pub fn scene3d_create_terrain_bytes_splat(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let data = in_bytes(input, &mut pos).to_vec();
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let args = decode_terrain_splat_input(input, &mut pos);
    pos.finish();
    let result = args.and_then(
        |(
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            normal_scales,
            terrain_tuning,
        )| {
            crate::externs::util::with_renderer_result(|r| {
                r.create_terrain_splat(
                    &data,
                    TerrainSplatOptions {
                        scale: [scale_x, scale_y, scale_z],
                        material: TerrainSplatMaterialOptions {
                            control_texture_id,
                            layer_texture_ids,
                            layer_normal_texture_ids,
                            layer_metallic_roughness_texture_ids,
                            uv_scales,
                            layer_normal_scales: normal_scales,
                            terrain_tuning,
                        },
                    },
                )
            })
        },
    );
    crate::externs::resource::encode_terrain_result_bytes(result)
}

fn decode_terrain_splat_input(
    input: &[u8],
    pos: &mut usize,
) -> Result<crate::externs::resource::TerrainSplatArgs, String> {
    let control_texture_id = in_value(input, pos) as u32;
    let layer_data = in_bytes(input, pos);
    crate::externs::resource::decode_terrain_splat_layer_data(control_texture_id, layer_data)
}

/// scene3d_terrainHeightAt(worldId, bodyId, x, z) → (float, bool)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene3d", "terrainHeightAt")]
pub fn scene3d_terrain_height_at(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let x = in_f64(input, &mut pos) as f32;
    let z = in_f64(input, &mut pos) as f32;
    pos.finish();
    let mut out = Vec::new();
    match crate::terrain::height_at(world_id, body_id, x, z) {
        Some(h) => {
            out_value_f64(&mut out, h as f64);
            out_value_bool(&mut out, true);
        }
        None => {
            out_value_f64(&mut out, 0.0);
            out_value_bool(&mut out, false);
        }
    }
    out
}

/// scene3d_createPlaneMesh(width, depth, subX, subZ) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createPlaneMesh")]
pub fn scene3d_create_plane_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let width = in_f64(input, &mut pos) as f32;
    let depth = in_f64(input, &mut pos) as f32;
    let sub_x = in_value(input, &mut pos) as u32;
    let sub_z = in_value(input, &mut pos) as u32;
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createPlaneMesh", |r| {
        r.create_plane(width, depth, sub_x, sub_z)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCubeMesh() → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createCubeMesh")]
pub fn scene3d_create_cube_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createCubeMesh", |r| r.create_cube());
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createRoundedBoxMesh(bevelRadius, segments) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createRoundedBoxMesh")]
pub fn scene3d_create_rounded_box_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let bevel_radius = in_f64(input, &mut pos) as f32;
    let segments = in_value(input, &mut pos) as u32;
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createRoundedBoxMesh", |r| {
        r.create_rounded_box(bevel_radius, segments)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createSphereMesh(segments) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createSphereMesh")]
pub fn scene3d_create_sphere_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let segments = in_value(input, &mut pos) as u32;
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createSphereMesh", |r| {
        r.create_sphere(segments)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCylinderMesh(segments) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createCylinderMesh")]
pub fn scene3d_create_cylinder_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let segments = in_value(input, &mut pos) as u32;
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createCylinderMesh", |r| {
        r.create_cylinder(segments)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createConeMesh(segments) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createConeMesh")]
pub fn scene3d_create_cone_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let segments = in_value(input, &mut pos) as u32;
    pos.finish();
    let id =
        crate::externs::util::with_renderer_or_panic("createConeMesh", |r| r.create_cone(segments));
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createWedgeMesh() → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createWedgeMesh")]
pub fn scene3d_create_wedge_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createWedgeMesh", |r| r.create_wedge());
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

/// scene3d_createCapsuleMesh(segments, halfHeight, radius) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay", "createCapsuleMesh")]
pub fn scene3d_create_capsule_mesh(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let segments = in_value(input, &mut pos) as u32;
    let half_height = in_f64(input, &mut pos) as f32;
    let radius = in_f64(input, &mut pos) as f32;
    pos.finish();
    let id = crate::externs::util::with_renderer_or_panic("createCapsuleMesh", |r| {
        r.create_capsule(segments, half_height, radius)
    });
    let mut out = Vec::new();
    out_value_u64(&mut out, id as u64);
    out
}

// ── Audio externs ─────────────────────────────────────────────────────────────

/// audioLoadFile(path string) → (uint32, error)
#[vo_ext::vo_wasm_bindgen_export("voplay", "audioLoadFile")]
pub fn audio_load_file(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let path = in_str(input, &mut pos).to_string();
    pos.finish();
    let result = crate::file_io::read_bytes(&path)
        .map_err(|e| format!("audio load error: {e}"))
        .and_then(|data| {
            vo_vogui::audio::with_global_audio_result(|engine| engine.load_bytes(data))
        });
    let mut out = Vec::new();
    out_u32_handle_result(&mut out, result);
    out
}
