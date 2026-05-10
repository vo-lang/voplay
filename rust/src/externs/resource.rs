//! Font and model load/free externs (voplay root package).

use std::sync::{Mutex, OnceLock};

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use crate::file_io;
use crate::font_manager::FontManager;
use crate::model_loader::{LevelNode, LevelNodeKind, ModelGeometryData, TerrainMaterialTuning};

use super::util::{
    ret_bytes, with_renderer_or_panic, with_renderer_result, write_bytes_result,
    write_u32_handle_result,
};
use super::with_renderer;

static HEADLESS_FONT_MANAGER: OnceLock<Result<Mutex<FontManager>, String>> = OnceLock::new();

pub(crate) fn with_headless_font_manager_pub<R>(
    f: impl FnOnce(&mut FontManager) -> R,
) -> Result<R, String> {
    with_headless_font_manager(f)
}

fn with_headless_font_manager<R>(f: impl FnOnce(&mut FontManager) -> R) -> Result<R, String> {
    let manager = HEADLESS_FONT_MANAGER.get_or_init(|| FontManager::new().map(Mutex::new));
    let manager = manager.as_ref().map_err(|error| error.clone())?;
    let mut manager = manager.lock().unwrap();
    Ok(f(&mut manager))
}

// --- Font externs ---

#[vo_fn("voplay", "loadFont")]
pub fn load_font(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let result = match with_renderer(|r| r.load_font(&path)) {
        Ok(result) => result,
        Err(_) => {
            with_headless_font_manager(|fonts| fonts.load_file(&path)).and_then(|result| result)
        }
    };
    write_u32_handle_result(call, 0, 1, result);
    ExternResult::Ok
}

#[vo_fn("voplay", "loadFontBytes")]
pub fn load_font_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    let result = match with_renderer(|r| r.load_font_bytes(data.clone())) {
        Ok(result) => result,
        Err(_) => {
            with_headless_font_manager(|fonts| fonts.load_bytes(data)).and_then(|result| result)
        }
    };
    write_u32_handle_result(call, 0, 1, result);
    ExternResult::Ok
}

#[vo_fn("voplay", "freeFont")]
pub fn free_font(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    if with_renderer(|r| r.free_font(id)).is_err() {
        with_headless_font_manager(|fonts| fonts.free(id)).unwrap_or_else(|msg| panic!("{}", msg));
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "measureText")]
pub fn measure_text(call: &mut ExternCallContext) -> ExternResult {
    let font_id = call.arg_u64(0) as u32;
    let text = call.arg_str(1);
    let size = call.arg_f64(2) as f32;
    let (w, h) = match with_renderer(|r| r.measure_text(font_id, text, size)) {
        Ok(result) => result,
        Err(_) => with_headless_font_manager(|fonts| fonts.measure_text(font_id, text, size))
            .unwrap_or_else(|msg| panic!("{}", msg)),
    };
    call.ret_f64(0, w as f64);
    call.ret_f64(1, h as f64);
    ExternResult::Ok
}

// --- Model externs ---

#[vo_fn("voplay", "loadModel")]
pub fn load_model(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    write_u32_handle_result(call, 0, 1, with_renderer_result(|r| r.load_model(&path)));
    ExternResult::Ok
}

#[vo_fn("voplay", "loadModelBytes")]
pub fn load_model_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    write_u32_handle_result(
        call,
        0,
        1,
        with_renderer_result(|r| r.load_model_bytes(&data)),
    );
    ExternResult::Ok
}

#[vo_fn("voplay", "freeModel")]
pub fn free_model(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let _ = with_renderer(|r| r.free_model(id));
    ExternResult::Ok
}

#[vo_fn("voplay", "modelBounds")]
pub fn model_bounds(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    match with_renderer(|r| r.model_bounds(id)) {
        Ok(Some((min, max))) => {
            call.ret_f64(0, min[0] as f64);
            call.ret_f64(1, min[1] as f64);
            call.ret_f64(2, min[2] as f64);
            call.ret_f64(3, max[0] as f64);
            call.ret_f64(4, max[1] as f64);
            call.ret_f64(5, max[2] as f64);
            call.ret_bool(6, true);
        }
        _ => {
            call.ret_f64(0, 0.0);
            call.ret_f64(1, 0.0);
            call.ret_f64(2, 0.0);
            call.ret_f64(3, 0.0);
            call.ret_f64(4, 0.0);
            call.ret_f64(5, 0.0);
            call.ret_bool(6, false);
        }
    }
    ExternResult::Ok
}

pub(crate) fn encode_model_geometry_bytes(geometry: &ModelGeometryData) -> Vec<u8> {
    let vertex_count = geometry.positions.len();
    let index_count = geometry.indices.len();
    let triangle_count = index_count / 3;
    let normals: &[[f32; 3]] = if geometry.normals.len() == vertex_count {
        &geometry.normals
    } else {
        &[]
    };
    let uvs: &[[f32; 2]] = if geometry.uvs.len() == vertex_count {
        &geometry.uvs
    } else {
        &[]
    };
    let colors: &[[f32; 4]] = if geometry.colors.len() == vertex_count {
        &geometry.colors
    } else {
        &[]
    };
    let default_material =
        crate::model_loader::MeshMaterial::standard([1.0, 1.0, 1.0, 1.0], None, 1.0);
    let materials: &[crate::model_loader::MeshMaterial] = if geometry.materials.is_empty() {
        std::slice::from_ref(&default_material)
    } else {
        &geometry.materials
    };
    let flags = (if normals.is_empty() { 0 } else { 1 })
        | (if uvs.is_empty() { 0 } else { 2 })
        | (if colors.is_empty() { 0 } else { 4 })
        | 8;
    let material_record_bytes = 84usize;
    let mut out = Vec::with_capacity(
        24 + vertex_count * 48
            + materials.len() * material_record_bytes
            + index_count * 4
            + triangle_count * 4,
    );
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(vertex_count as u32).to_le_bytes());
    out.extend_from_slice(&(index_count as u32).to_le_bytes());
    out.extend_from_slice(&(flags as u32).to_le_bytes());
    out.extend_from_slice(&(materials.len() as u32).to_le_bytes());
    out.extend_from_slice(&(triangle_count as u32).to_le_bytes());
    for index in 0..vertex_count {
        let pos = geometry.positions[index];
        out.extend_from_slice(&pos[0].to_le_bytes());
        out.extend_from_slice(&pos[1].to_le_bytes());
        out.extend_from_slice(&pos[2].to_le_bytes());
        let normal = normals.get(index).copied().unwrap_or([0.0, 1.0, 0.0]);
        out.extend_from_slice(&normal[0].to_le_bytes());
        out.extend_from_slice(&normal[1].to_le_bytes());
        out.extend_from_slice(&normal[2].to_le_bytes());
        let uv = uvs.get(index).copied().unwrap_or([0.0, 0.0]);
        out.extend_from_slice(&uv[0].to_le_bytes());
        out.extend_from_slice(&uv[1].to_le_bytes());
        let color = colors.get(index).copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
        out.extend_from_slice(&color[0].to_le_bytes());
        out.extend_from_slice(&color[1].to_le_bytes());
        out.extend_from_slice(&color[2].to_le_bytes());
        out.extend_from_slice(&color[3].to_le_bytes());
    }
    for material in materials {
        for value in material.base_color {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in material.emissive_factor {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            material.metallic,
            material.roughness,
            material.normal_scale,
            material.detail_strength,
            material.macro_blend,
            material.roughness_response,
            material.toon_ramp_response,
            material.uv_scales[0],
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            material.texture_id.unwrap_or(0),
            material.normal_texture_id.unwrap_or(0),
            material.metallic_roughness_texture_id.unwrap_or(0),
            material.emissive_texture_id.unwrap_or(0),
            material.toon_ramp_texture_id.unwrap_or(0),
            material.mask_texture_id.unwrap_or(0),
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in &geometry.indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    for triangle_index in 0..triangle_count {
        let material_index = geometry
            .triangle_materials
            .get(triangle_index)
            .copied()
            .unwrap_or(0)
            .min((materials.len() - 1) as u32);
        out.extend_from_slice(&material_index.to_le_bytes());
    }
    out
}

#[vo_fn("voplay", "modelGeometryBytes")]
pub fn model_geometry_bytes(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let geometry = with_renderer_or_panic("modelGeometryBytes", |renderer| {
        renderer.get_model_geometry(id)
    });
    let geometry =
        geometry.unwrap_or_else(|| panic!("modelGeometryBytes: model not found: {}", id));
    let data = encode_model_geometry_bytes(&geometry);
    ret_bytes(call, 0, &data);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "bakeImpostorAtlasBytes")]
pub fn scene3d_bake_impostor_atlas_bytes(call: &mut ExternCallContext) -> ExternResult {
    let request = call.arg_bytes(0).to_vec();
    write_bytes_result(
        call,
        0,
        1,
        crate::impostor_baker::bake_impostor_atlas_bytes(&request),
    );
    ExternResult::Ok
}

pub(crate) fn serialize_level_nodes(nodes: &[LevelNode]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    for node in nodes {
        let name = node.name.as_bytes();
        assert!(
            name.len() <= u16::MAX as usize,
            "voplay: level node name too long: {}",
            node.name
        );
        buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
        buf.extend_from_slice(name);
        buf.push(match node.kind {
            LevelNodeKind::Entity => 0,
            LevelNodeKind::Terrain => 1,
        });
        buf.extend_from_slice(&node.model_id.to_le_bytes());
        for value in [
            node.position[0] as f64,
            node.position[1] as f64,
            node.position[2] as f64,
            node.rotation[0] as f64,
            node.rotation[1] as f64,
            node.rotation[2] as f64,
            node.rotation[3] as f64,
            node.scale[0] as f64,
            node.scale[1] as f64,
            node.scale[2] as f64,
            node.aabb_min[0] as f64,
            node.aabb_min[1] as f64,
            node.aabb_min[2] as f64,
            node.aabb_max[0] as f64,
            node.aabb_max[1] as f64,
            node.aabb_max[2] as f64,
        ] {
            buf.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(terrain) = node.terrain.as_ref() {
            buf.extend_from_slice(&terrain.rows.to_le_bytes());
            buf.extend_from_slice(&terrain.cols.to_le_bytes());
            for value in [
                terrain.scale[0] as f64,
                terrain.scale[1] as f64,
                terrain.scale[2] as f64,
            ] {
                buf.extend_from_slice(&value.to_le_bytes());
            }
            buf.extend_from_slice(&terrain.layer.to_le_bytes());
            buf.extend_from_slice(&terrain.mask.to_le_bytes());
            buf.extend_from_slice(&(terrain.friction as f64).to_le_bytes());
            buf.extend_from_slice(&(terrain.restitution as f64).to_le_bytes());
            let height_bytes = terrain_heights_to_bytes(&terrain.heights);
            buf.extend_from_slice(&(height_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(&height_bytes);
        }
    }
    buf
}

fn terrain_heights_to_bytes(heights: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(heights.len() * 4);
    for height in heights {
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    bytes
}

pub(crate) fn encode_terrain_result_bytes(
    result: Result<crate::terrain::TerrainData, String>,
) -> Vec<u8> {
    // TAG constants from island_bindgen
    const TAG_VALUE: u8 = 0xE2;
    const TAG_BYTES: u8 = 0xE3;
    const TAG_NIL_ERROR: u8 = 0xE0;
    const TAG_ERROR_STR: u8 = 0xE1;
    let mut out = Vec::new();
    match result {
        Ok(data) => {
            out.push(TAG_VALUE);
            out.extend_from_slice(&(data.model_id as u64).to_le_bytes());
            out.push(TAG_VALUE);
            out.extend_from_slice(&(data.rows as u64).to_le_bytes());
            out.push(TAG_VALUE);
            out.extend_from_slice(&(data.cols as u64).to_le_bytes());
            let height_bytes = terrain_heights_to_bytes(&data.heights);
            out.push(TAG_BYTES);
            out.extend_from_slice(&(height_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&height_bytes);
            out.push(TAG_NIL_ERROR);
        }
        Err(msg) => {
            out.push(TAG_VALUE);
            out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_VALUE);
            out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_VALUE);
            out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_BYTES);
            out.extend_from_slice(&0u32.to_le_bytes());
            let bytes = msg.as_bytes();
            let len = bytes.len().min(65535) as u16;
            out.push(TAG_ERROR_STR);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&bytes[..len as usize]);
        }
    }
    out
}

fn write_terrain_result(
    call: &mut ExternCallContext,
    result: Result<crate::terrain::TerrainData, String>,
) {
    match result {
        Ok(data) => {
            call.ret_u64(0, data.model_id as u64);
            call.ret_u64(1, data.rows as u64);
            call.ret_u64(2, data.cols as u64);
            let height_bytes = terrain_heights_to_bytes(&data.heights);
            ret_bytes(call, 3, &height_bytes);
            write_nil_error(call, 4);
        }
        Err(msg) => {
            call.ret_u64(0, 0);
            call.ret_u64(1, 0);
            call.ret_u64(2, 0);
            ret_bytes(call, 3, &[]);
            write_error_to(call, 4, &msg);
        }
    }
}

pub(crate) type TerrainSplatArgs = (
    u32,
    [u32; 4],
    [u32; 4],
    [u32; 4],
    [f32; 4],
    [f32; 4],
    TerrainMaterialTuning,
);

fn read_layer_u32(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > data.len() {
        return Err("terrain splat layer data is truncated".to_string());
    }
    let value = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(value)
}

fn read_layer_f64(data: &[u8], pos: &mut usize) -> Result<f64, String> {
    if *pos + 8 > data.len() {
        return Err("terrain splat layer data is truncated".to_string());
    }
    let value = f64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(value)
}

pub(crate) fn decode_terrain_splat_layer_data(
    control_texture_id: u32,
    layer_data: &[u8],
) -> Result<TerrainSplatArgs, String> {
    const LAYER_COUNT: usize = 4;
    const LAYER_BYTES: usize = 4 + 4 + 4 + 8 + 8;
    const EXPECTED_BYTES: usize = LAYER_COUNT * LAYER_BYTES;
    const TUNING_BYTES: usize = 16 * 8;
    const EXPECTED_BYTES_WITH_TUNING: usize = EXPECTED_BYTES + TUNING_BYTES;

    if control_texture_id == 0 {
        return Err("terrain splat control texture must be non-zero".to_string());
    }
    if layer_data.len() != EXPECTED_BYTES_WITH_TUNING {
        return Err(format!(
            "terrain splat layer data must be {} bytes, got {}",
            EXPECTED_BYTES_WITH_TUNING,
            layer_data.len()
        ));
    }

    let mut pos = 0usize;
    let mut layer_texture_ids = [0u32; 4];
    let mut layer_normal_texture_ids = [0u32; 4];
    let mut layer_metallic_roughness_texture_ids = [0u32; 4];
    let mut uv_scales = [1.0f32; 4];
    let mut normal_scales = [0.0f32; 4];
    for index in 0..4 {
        let texture_id = read_layer_u32(layer_data, &mut pos)?;
        if texture_id == 0 {
            return Err(format!(
                "terrain splat layer {} texture must be non-zero",
                index
            ));
        }
        let normal_texture_id = read_layer_u32(layer_data, &mut pos)?;
        let metallic_roughness_texture_id = read_layer_u32(layer_data, &mut pos)?;
        let uv_scale = read_layer_f64(layer_data, &mut pos)? as f32;
        if !uv_scale.is_finite() || uv_scale <= 0.0 {
            return Err(format!("terrain splat layer {} uvScale must be > 0", index));
        }
        let normal_scale = read_layer_f64(layer_data, &mut pos)? as f32;
        if !normal_scale.is_finite() || normal_scale < 0.0 {
            return Err(format!(
                "terrain splat layer {} normalScale must be >= 0",
                index
            ));
        }
        layer_texture_ids[index] = texture_id;
        layer_normal_texture_ids[index] = normal_texture_id;
        layer_metallic_roughness_texture_ids[index] = metallic_roughness_texture_id;
        uv_scales[index] = uv_scale;
        normal_scales[index] = normal_scale;
    }
    let terrain_tuning = {
        let mut tuning = TerrainMaterialTuning {
            macro_scale: read_layer_f64(layer_data, &mut pos)? as f32,
            macro_strength: read_layer_f64(layer_data, &mut pos)? as f32,
            detail_near: read_layer_f64(layer_data, &mut pos)? as f32,
            detail_far: read_layer_f64(layer_data, &mut pos)? as f32,
            slope_start: read_layer_f64(layer_data, &mut pos)? as f32,
            slope_end: read_layer_f64(layer_data, &mut pos)? as f32,
            slope_dirt_strength: read_layer_f64(layer_data, &mut pos)? as f32,
            slope_rock_strength: read_layer_f64(layer_data, &mut pos)? as f32,
            anti_tile_strength: read_layer_f64(layer_data, &mut pos)? as f32,
            detail_strength: read_layer_f64(layer_data, &mut pos)? as f32,
            normal_near: read_layer_f64(layer_data, &mut pos)? as f32,
            normal_far: read_layer_f64(layer_data, &mut pos)? as f32,
            ..TerrainMaterialTuning::default()
        };
        tuning.height_blend_strength = read_layer_f64(layer_data, &mut pos)? as f32;
        tuning.height_low = read_layer_f64(layer_data, &mut pos)? as f32;
        tuning.height_high = read_layer_f64(layer_data, &mut pos)? as f32;
        tuning.curvature_strength = read_layer_f64(layer_data, &mut pos)? as f32;
        tuning.normalized()?
    };

    Ok((
        control_texture_id,
        layer_texture_ids,
        layer_normal_texture_ids,
        layer_metallic_roughness_texture_ids,
        uv_scales,
        normal_scales,
        terrain_tuning,
    ))
}

fn decode_terrain_splat_args(
    call: &mut ExternCallContext,
    base: usize,
) -> Result<TerrainSplatArgs, String> {
    let control_texture_id = call.arg_u64(base as u16) as u32;
    let layer_data = call.arg_bytes((base + 1) as u16);
    decode_terrain_splat_layer_data(control_texture_id, layer_data)
}

#[vo_fn("voplay/scene3d", "loadLevel")]
pub fn load_level(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let result =
        with_renderer_result(|r| r.load_level(&path)).map(|nodes| serialize_level_nodes(&nodes));
    write_bytes_result(call, 0, 1, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "createTerrain")]
pub fn create_terrain(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let scale_x = call.arg_f64(1) as f32;
    let scale_y = call.arg_f64(2) as f32;
    let scale_z = call.arg_f64(3) as f32;
    let uv_scale = call.arg_f64(4) as f32;
    let texture_id = match call.arg_u64(5) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_texture_id = match call.arg_u64(6) as u32 {
        0 => None,
        id => Some(id),
    };
    let metallic_roughness_texture_id = match call.arg_u64(7) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_scale = call.arg_f64(8) as f32;
    let roughness = call.arg_f64(9) as f32;
    let metallic = call.arg_f64(10) as f32;
    let result = file_io::read_bytes(&path)
        .map_err(|e| format!("terrain: read {}: {}", path, e))
        .and_then(|data| {
            with_renderer_result(|r| {
                r.create_terrain(
                    &data,
                    scale_x,
                    scale_y,
                    scale_z,
                    uv_scale,
                    texture_id,
                    normal_texture_id,
                    metallic_roughness_texture_id,
                    normal_scale,
                    roughness,
                    metallic,
                )
            })
        });
    write_terrain_result(call, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "createTerrainSplat")]
pub fn create_terrain_splat(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let scale_x = call.arg_f64(1) as f32;
    let scale_y = call.arg_f64(2) as f32;
    let scale_z = call.arg_f64(3) as f32;
    let args = decode_terrain_splat_args(call, 4);
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
            file_io::read_bytes(&path)
                .map_err(|e| format!("terrain: read {}: {}", path, e))
                .and_then(|data| {
                    with_renderer_result(|r| {
                        r.create_terrain_splat(
                            &data,
                            scale_x,
                            scale_y,
                            scale_z,
                            control_texture_id,
                            layer_texture_ids,
                            layer_normal_texture_ids,
                            layer_metallic_roughness_texture_ids,
                            uv_scales,
                            normal_scales,
                            terrain_tuning,
                        )
                    })
                })
        },
    );
    write_terrain_result(call, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "createTerrainSplatModel")]
pub fn create_terrain_splat_model(call: &mut ExternCallContext) -> ExternResult {
    let model_id = call.arg_u64(0) as u32;
    let args = decode_terrain_splat_args(call, 1);
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
            with_renderer_result(|r| {
                r.create_terrain_splat_model(
                    model_id,
                    control_texture_id,
                    layer_texture_ids,
                    layer_normal_texture_ids,
                    layer_metallic_roughness_texture_ids,
                    uv_scales,
                    normal_scales,
                    terrain_tuning,
                )
            })
        },
    );
    write_u32_handle_result(call, 0, 1, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "createTerrainBytes")]
pub fn create_terrain_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    let scale_x = call.arg_f64(1) as f32;
    let scale_y = call.arg_f64(2) as f32;
    let scale_z = call.arg_f64(3) as f32;
    let uv_scale = call.arg_f64(4) as f32;
    let texture_id = match call.arg_u64(5) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_texture_id = match call.arg_u64(6) as u32 {
        0 => None,
        id => Some(id),
    };
    let metallic_roughness_texture_id = match call.arg_u64(7) as u32 {
        0 => None,
        id => Some(id),
    };
    let normal_scale = call.arg_f64(8) as f32;
    let roughness = call.arg_f64(9) as f32;
    let metallic = call.arg_f64(10) as f32;
    let result = with_renderer_result(|r| {
        r.create_terrain(
            &data,
            scale_x,
            scale_y,
            scale_z,
            uv_scale,
            texture_id,
            normal_texture_id,
            metallic_roughness_texture_id,
            normal_scale,
            roughness,
            metallic,
        )
    });
    write_terrain_result(call, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "createTerrainBytesSplat")]
pub fn create_terrain_bytes_splat(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    let scale_x = call.arg_f64(1) as f32;
    let scale_y = call.arg_f64(2) as f32;
    let scale_z = call.arg_f64(3) as f32;
    let args = decode_terrain_splat_args(call, 4);
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
            with_renderer_result(|r| {
                r.create_terrain_splat(
                    &data,
                    scale_x,
                    scale_y,
                    scale_z,
                    control_texture_id,
                    layer_texture_ids,
                    layer_normal_texture_ids,
                    layer_metallic_roughness_texture_ids,
                    uv_scales,
                    normal_scales,
                    terrain_tuning,
                )
            })
        },
    );
    write_terrain_result(call, result);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "terrainHeightAt")]
pub fn terrain_height_at(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let x = call.arg_f64(2) as f32;
    let z = call.arg_f64(3) as f32;
    match crate::terrain::height_at(world_id, body_id, x, z) {
        Some(height) => {
            call.ret_f64(0, height as f64);
            call.ret_bool(1, true);
        }
        None => {
            call.ret_f64(0, 0.0);
            call.ret_bool(1, false);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "createPlaneMesh")]
pub fn create_plane_mesh(call: &mut ExternCallContext) -> ExternResult {
    let width = call.arg_f64(0) as f32;
    let depth = call.arg_f64(1) as f32;
    let sub_x = call.arg_u64(2) as u32;
    let sub_z = call.arg_u64(3) as u32;
    let id = with_renderer_or_panic("createPlaneMesh", |r| {
        r.create_plane(width, depth, sub_x, sub_z)
    });
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createCubeMesh")]
pub fn create_cube_mesh(call: &mut ExternCallContext) -> ExternResult {
    let id = with_renderer_or_panic("createCubeMesh", |r| r.create_cube());
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createRoundedBoxMesh")]
pub fn create_rounded_box_mesh(call: &mut ExternCallContext) -> ExternResult {
    let bevel_radius = call.arg_f64(0) as f32;
    let segments = call.arg_u64(1) as u32;
    let id = with_renderer_or_panic("createRoundedBoxMesh", |r| {
        r.create_rounded_box(bevel_radius, segments)
    });
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createSphereMesh")]
pub fn create_sphere_mesh(call: &mut ExternCallContext) -> ExternResult {
    let segments = call.arg_u64(0) as u32;
    let id = with_renderer_or_panic("createSphereMesh", |r| r.create_sphere(segments));
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createCylinderMesh")]
pub fn create_cylinder_mesh(call: &mut ExternCallContext) -> ExternResult {
    let segments = call.arg_u64(0) as u32;
    let id = with_renderer_or_panic("createCylinderMesh", |r| r.create_cylinder(segments));
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createConeMesh")]
pub fn create_cone_mesh(call: &mut ExternCallContext) -> ExternResult {
    let segments = call.arg_u64(0) as u32;
    let id = with_renderer_or_panic("createConeMesh", |r| r.create_cone(segments));
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createWedgeMesh")]
pub fn create_wedge_mesh(call: &mut ExternCallContext) -> ExternResult {
    let id = with_renderer_or_panic("createWedgeMesh", |r| r.create_wedge());
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createCapsuleMesh")]
pub fn create_capsule_mesh(call: &mut ExternCallContext) -> ExternResult {
    let segments = call.arg_u64(0) as u32;
    let half_height = call.arg_f64(1) as f32;
    let radius = call.arg_f64(2) as f32;
    let id = with_renderer_or_panic("createCapsuleMesh", |r| {
        r.create_capsule(segments, half_height, radius)
    });
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createRawMesh")]
pub fn create_raw_mesh(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    write_u32_handle_result(
        call,
        0,
        1,
        with_renderer_result(|r| r.create_raw_mesh(&data)),
    );
    ExternResult::Ok
}

#[cfg(test)]
mod tests {
    use super::decode_terrain_splat_layer_data;

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_f64(out: &mut Vec<u8>, value: f64) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn terrain_splat_payload(tuning_fields: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for layer in 0..4u32 {
            push_u32(&mut out, 11 + layer);
            push_u32(&mut out, 21 + layer);
            push_u32(&mut out, 31 + layer);
            push_f64(&mut out, 1.0 + layer as f64);
            push_f64(&mut out, 0.25 + layer as f64 * 0.1);
        }
        let values = [
            1.25, 0.85, 120.0, 420.0, 0.12, 0.68, 0.4, 0.7, 0.9, 1.1, 90.0, 460.0, 0.35, -2.0,
            18.0, 0.45,
        ];
        for value in values.iter().take(tuning_fields) {
            push_f64(&mut out, *value);
        }
        out
    }

    #[test]
    fn terrain_splat_requires_single_current_payload_shape() {
        let layer_only = terrain_splat_payload(0);
        assert!(decode_terrain_splat_layer_data(9, &layer_only).is_err());

        let twelve_field_tuning_shape = terrain_splat_payload(12);
        assert!(decode_terrain_splat_layer_data(9, &twelve_field_tuning_shape).is_err());

        let current = terrain_splat_payload(16);
        let decoded = decode_terrain_splat_layer_data(9, &current).unwrap();
        assert_eq!(decoded.0, 9);
        assert_eq!(decoded.1, [11, 12, 13, 14]);
        assert_eq!(decoded.2, [21, 22, 23, 24]);
        assert_eq!(decoded.3, [31, 32, 33, 34]);
        assert!((decoded.4[3] - 4.0).abs() < 0.0001);
        assert!((decoded.6.height_blend_strength - 0.35).abs() < 0.0001);
        assert!((decoded.6.curvature_strength - 0.45).abs() < 0.0001);
    }
}
