use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::math3d::Vec3;
use crate::model_loader::{MeshMaterial, MeshVertex, ModelId, ModelManager};

#[derive(Clone)]
pub struct TerrainData {
    pub model_id: ModelId,
    pub heights: Vec<f32>,
    pub rows: u32,
    pub cols: u32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub scale_z: f32,
    pub origin: Vec3,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TerrainKey {
    world_id: u32,
    body_id: u32,
}

static TERRAINS: LazyLock<Mutex<HashMap<TerrainKey, TerrainData>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn height_index(cols: u32, row: u32, col: u32) -> usize {
    (row * cols + col) as usize
}

fn sample_height(heights: &[f32], rows: u32, cols: u32, row: u32, col: u32) -> f32 {
    assert!(row < rows, "terrain row out of range: {} >= {}", row, rows);
    assert!(col < cols, "terrain col out of range: {} >= {}", col, cols);
    heights[height_index(cols, row, col)]
}

fn height_gradient_x(heights: &[f32], rows: u32, cols: u32, row: u32, col: u32, cell_x: f32) -> f32 {
    if col == 0 {
        (sample_height(heights, rows, cols, row, 1) - sample_height(heights, rows, cols, row, 0)) / cell_x
    } else if col + 1 == cols {
        (sample_height(heights, rows, cols, row, col) - sample_height(heights, rows, cols, row, col - 1)) / cell_x
    } else {
        (sample_height(heights, rows, cols, row, col + 1) - sample_height(heights, rows, cols, row, col - 1))
            / (2.0 * cell_x)
    }
}

fn height_gradient_z(heights: &[f32], rows: u32, cols: u32, row: u32, col: u32, cell_z: f32) -> f32 {
    if row == 0 {
        (sample_height(heights, rows, cols, 1, col) - sample_height(heights, rows, cols, 0, col)) / cell_z
    } else if row + 1 == rows {
        (sample_height(heights, rows, cols, row, col) - sample_height(heights, rows, cols, row - 1, col)) / cell_z
    } else {
        (sample_height(heights, rows, cols, row + 1, col) - sample_height(heights, rows, cols, row - 1, col))
            / (2.0 * cell_z)
    }
}

pub fn generate_terrain(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    model_manager: &mut ModelManager,
    image_data: &[u8],
    scale_x: f32,
    scale_y: f32,
    scale_z: f32,
    material: MeshMaterial,
) -> Result<TerrainData, String> {
    if scale_x <= 0.0 || scale_y <= 0.0 || scale_z <= 0.0 {
        return Err("terrain scale must be > 0".to_string());
    }

    let img = image::load_from_memory(image_data)
        .map_err(|e| format!("terrain heightmap decode: {}", e))?;
    let gray = img.to_luma8();
    let (cols, rows) = gray.dimensions();
    if rows < 2 || cols < 2 {
        return Err(format!("terrain heightmap must be at least 2x2, got {}x{}", cols, rows));
    }

    let heights: Vec<f32> = gray.pixels().map(|pixel| pixel.0[0] as f32 / 255.0).collect();
    let cell_x = scale_x / (cols - 1) as f32;
    let cell_z = scale_z / (rows - 1) as f32;

    let mut vertices = Vec::with_capacity((rows * cols) as usize);
    for row in 0..rows {
        for col in 0..cols {
            let idx = height_index(cols, row, col);
            let h = heights[idx] * scale_y;
            let x = col as f32 * cell_x - scale_x * 0.5;
            let z = row as f32 * cell_z - scale_z * 0.5;
            let dh_dx = height_gradient_x(&heights, rows, cols, row, col, cell_x) * scale_y;
            let dh_dz = height_gradient_z(&heights, rows, cols, row, col, cell_z) * scale_y;
            let normal = Vec3::new(-dh_dx, 1.0, -dh_dz).normalize().to_array();
            let u = col as f32 / (cols - 1) as f32;
            let v = row as f32 / (rows - 1) as f32;
            vertices.push(MeshVertex {
                position: [x, h, z],
                normal,
                uv: [u, v],
            });
        }
    }

    let mut indices = Vec::with_capacity(((rows - 1) * (cols - 1) * 6) as usize);
    for row in 0..(rows - 1) {
        for col in 0..(cols - 1) {
            let i0 = row * cols + col;
            let i1 = (row + 1) * cols + col;
            let i2 = i0 + 1;
            let i3 = i1 + 1;
            indices.extend_from_slice(&[i0, i1, i2, i2, i1, i3]);
        }
    }

    let model_id = model_manager.create_raw_with_material(
        device,
        queue,
        &vertices,
        &indices,
        material,
    );

    Ok(TerrainData {
        model_id,
        heights,
        rows,
        cols,
        scale_x,
        scale_y,
        scale_z,
        origin: Vec3::ZERO,
    })
}

pub fn store_terrain(world_id: u32, body_id: u32, origin: Vec3, mut data: TerrainData) {
    data.origin = origin;
    let mut terrains = TERRAINS.lock().unwrap();
    terrains.insert(TerrainKey { world_id, body_id }, data);
}

pub fn remove_terrain(world_id: u32, body_id: u32) {
    let mut terrains = TERRAINS.lock().unwrap();
    terrains.remove(&TerrainKey { world_id, body_id });
}

pub fn remove_world(world_id: u32) {
    let mut terrains = TERRAINS.lock().unwrap();
    terrains.retain(|key, _| key.world_id != world_id);
}

pub fn height_at(world_id: u32, body_id: u32, world_x: f32, world_z: f32) -> Option<f32> {
    let terrains = TERRAINS.lock().unwrap();
    let terrain = terrains.get(&TerrainKey { world_id, body_id })?;

    let local_x = world_x - terrain.origin.x;
    let local_z = world_z - terrain.origin.z;
    let max_col = terrain.cols as usize - 1;
    let max_row = terrain.rows as usize - 1;
    let gx = (local_x + terrain.scale_x * 0.5) / terrain.scale_x * max_col as f32;
    let gz = (local_z + terrain.scale_z * 0.5) / terrain.scale_z * max_row as f32;

    if gx < 0.0 || gz < 0.0 || gx > max_col as f32 || gz > max_row as f32 {
        return None;
    }

    let col0 = gx.floor().min((max_col - 1) as f32) as usize;
    let row0 = gz.floor().min((max_row - 1) as f32) as usize;
    let col1 = (col0 + 1).min(max_col);
    let row1 = (row0 + 1).min(max_row);
    let fx = gx - col0 as f32;
    let fz = gz - row0 as f32;

    let h00 = terrain.heights[row0 * terrain.cols as usize + col0] * terrain.scale_y;
    let h10 = terrain.heights[row0 * terrain.cols as usize + col1] * terrain.scale_y;
    let h01 = terrain.heights[row1 * terrain.cols as usize + col0] * terrain.scale_y;
    let h11 = terrain.heights[row1 * terrain.cols as usize + col1] * terrain.scale_y;

    let h0 = h00 + (h10 - h00) * fx;
    let h1 = h01 + (h11 - h01) * fx;
    Some(terrain.origin.y + h0 + (h1 - h0) * fz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(got: f32, want: f32, tol: f32) {
        assert!((got - want).abs() <= tol, "got {}, want {}", got, want);
    }

    #[test]
    fn height_at_bilinear_and_world_offset() {
        let data = TerrainData {
            model_id: 1,
            heights: vec![0.0, 1.0, 0.5, 0.25],
            rows: 2,
            cols: 2,
            scale_x: 4.0,
            scale_y: 8.0,
            scale_z: 6.0,
            origin: Vec3::ZERO,
        };
        store_terrain(9, 7, Vec3::new(10.0, 3.0, -2.0), data);

        let center = height_at(9, 7, 10.0, -2.0).expect("center sample must exist");
        assert_near(center, 3.0 + (0.0 + 8.0 + 4.0 + 2.0) * 0.25, 0.0001);

        let corner = height_at(9, 7, 12.0, 1.0).expect("corner sample must exist");
        assert_near(corner, 3.0 + 2.0, 0.0001);

        assert!(height_at(9, 7, 12.1, 1.0).is_none(), "outside x bounds must fail");
        remove_terrain(9, 7);
    }
}
