//! Font and model load/free externs (voplay root package).

use std::sync::{Mutex, OnceLock};

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use crate::file_io;
use crate::font_manager::FontManager;
use crate::model_loader::{LevelNode, LevelNodeKind};

use super::util::{
    ret_bytes,
    with_renderer_or_panic,
    with_renderer_result,
    write_bytes_result,
    write_u32_handle_result,
};
use super::with_renderer;

static HEADLESS_FONT_MANAGER: OnceLock<Result<Mutex<FontManager>, String>> = OnceLock::new();

pub(crate) fn with_headless_font_manager_pub<R>(f: impl FnOnce(&mut FontManager) -> R) -> Result<R, String> {
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
        Err(_) => with_headless_font_manager(|fonts| fonts.load_file(&path)).and_then(|result| result),
    };
    write_u32_handle_result(call, 0, 1, result);
    ExternResult::Ok
}

#[vo_fn("voplay", "loadFontBytes")]
pub fn load_font_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();
    let result = match with_renderer(|r| r.load_font_bytes(data.clone())) {
        Ok(result) => result,
        Err(_) => with_headless_font_manager(|fonts| fonts.load_bytes(data)).and_then(|result| result),
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
    write_u32_handle_result(call, 0, 1, with_renderer_result(|r| r.load_model_bytes(&data)));
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

pub(crate) fn encode_model_mesh_data_bytes(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + positions.len() * 12 + indices.len() * 4);
    out.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for pos in positions {
        out.extend_from_slice(&pos[0].to_le_bytes());
        out.extend_from_slice(&pos[1].to_le_bytes());
        out.extend_from_slice(&pos[2].to_le_bytes());
    }
    for index in indices {
        out.extend_from_slice(&(*index).to_le_bytes());
    }
    out
}

#[vo_fn("voplay", "modelMeshDataBytes")]
pub fn model_mesh_data_bytes(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;
    let mesh_data = with_renderer_or_panic("modelMeshDataBytes", |renderer| {
        renderer.get_model_mesh_data(id)
    });
    let (positions, indices) = mesh_data
        .unwrap_or_else(|| panic!("modelMeshDataBytes: model not found: {}", id));
    let data = encode_model_mesh_data_bytes(&positions, &indices);
    ret_bytes(call, 0, &data);
    ExternResult::Ok
}

pub(crate) fn serialize_level_nodes(nodes: &[LevelNode]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    for node in nodes {
        let name = node.name.as_bytes();
        assert!(name.len() <= u16::MAX as usize, "voplay: level node name too long: {}", node.name);
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

pub(crate) fn encode_terrain_result_bytes(result: Result<crate::terrain::TerrainData, String>) -> Vec<u8> {
    // TAG constants from island_bindgen
    const TAG_VALUE: u8 = 0xE2;
    const TAG_BYTES: u8 = 0xE3;
    const TAG_NIL_ERROR: u8 = 0xE0;
    const TAG_ERROR_STR: u8 = 0xE1;
    let mut out = Vec::new();
    match result {
        Ok(data) => {
            out.push(TAG_VALUE); out.extend_from_slice(&(data.model_id as u64).to_le_bytes());
            out.push(TAG_VALUE); out.extend_from_slice(&(data.rows as u64).to_le_bytes());
            out.push(TAG_VALUE); out.extend_from_slice(&(data.cols as u64).to_le_bytes());
            let height_bytes = terrain_heights_to_bytes(&data.heights);
            out.push(TAG_BYTES);
            out.extend_from_slice(&(height_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&height_bytes);
            out.push(TAG_NIL_ERROR);
        }
        Err(msg) => {
            out.push(TAG_VALUE); out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_VALUE); out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_VALUE); out.extend_from_slice(&0u64.to_le_bytes());
            out.push(TAG_BYTES); out.extend_from_slice(&0u32.to_le_bytes());
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

fn decode_terrain_splat_args(
    call: &mut ExternCallContext,
    base: usize,
) -> Result<(u32, [u32; 4], [f32; 4]), String> {
    let control_texture_id = call.arg_u64(base as u16) as u32;
    if control_texture_id == 0 {
        return Err("terrain splat control texture must be non-zero".to_string());
    }
    let mut layer_texture_ids = [0u32; 4];
    let mut uv_scales = [1.0f32; 4];
    for index in 0..4 {
        let texture_id = call.arg_u64((base + 1 + index * 2) as u16) as u32;
        if texture_id == 0 {
            return Err(format!("terrain splat layer {} texture must be non-zero", index));
        }
        let uv_scale = call.arg_f64((base + 2 + index * 2) as u16) as f32;
        if uv_scale <= 0.0 {
            return Err(format!("terrain splat layer {} uvScale must be > 0", index));
        }
        layer_texture_ids[index] = texture_id;
        uv_scales[index] = uv_scale;
    }
    Ok((control_texture_id, layer_texture_ids, uv_scales))
}

#[vo_fn("voplay/scene3d", "loadLevel")]
pub fn load_level(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let result = with_renderer_result(|r| r.load_level(&path)).map(|nodes| serialize_level_nodes(&nodes));
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
    let result = file_io::read_bytes(&path)
        .map_err(|e| format!("terrain: read {}: {}", path, e))
        .and_then(|data| with_renderer_result(|r| r.create_terrain(&data, scale_x, scale_y, scale_z, uv_scale, texture_id)));
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
    let result = args.and_then(|(control_texture_id, layer_texture_ids, uv_scales)| {
        file_io::read_bytes(&path)
            .map_err(|e| format!("terrain: read {}: {}", path, e))
            .and_then(|data| with_renderer_result(|r| r.create_terrain_splat(&data, scale_x, scale_y, scale_z, control_texture_id, layer_texture_ids, uv_scales)))
    });
    write_terrain_result(call, result);
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
    let result = with_renderer_result(|r| r.create_terrain(&data, scale_x, scale_y, scale_z, uv_scale, texture_id));
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
    let result = args.and_then(|(control_texture_id, layer_texture_ids, uv_scales)| {
        with_renderer_result(|r| r.create_terrain_splat(&data, scale_x, scale_y, scale_z, control_texture_id, layer_texture_ids, uv_scales))
    });
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
    let id = with_renderer_or_panic("createPlaneMesh", |r| r.create_plane(width, depth, sub_x, sub_z));
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay", "createCubeMesh")]
pub fn create_cube_mesh(call: &mut ExternCallContext) -> ExternResult {
    let id = with_renderer_or_panic("createCubeMesh", |r| r.create_cube());
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

#[vo_fn("voplay", "createCapsuleMesh")]
pub fn create_capsule_mesh(call: &mut ExternCallContext) -> ExternResult {
    let segments = call.arg_u64(0) as u32;
    let half_height = call.arg_f64(1) as f32;
    let radius = call.arg_f64(2) as f32;
    let id = with_renderer_or_panic("createCapsuleMesh", |r| r.create_capsule(segments, half_height, radius));
    call.ret_u64(0, id as u64);
    ExternResult::Ok
}
