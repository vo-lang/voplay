use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
struct Vec2 {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy, Debug)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Material {
    base_color: Color,
    emissive: Vec3,
    metallic: f32,
    roughness: f32,
    normal_scale: f32,
    detail_strength: f32,
    macro_blend: f32,
    roughness_response: f32,
    toon_ramp_response: f32,
    uv_scale: f32,
    albedo: u32,
    normal: u32,
    metallic_roughness: u32,
    emissive_map: u32,
    _toon_ramp: u32,
    mask: u32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Color::default(),
            emissive: Vec3::default(),
            metallic: 0.0,
            roughness: 0.58,
            normal_scale: 1.0,
            detail_strength: 1.0,
            macro_blend: 0.0,
            roughness_response: 1.0,
            toon_ramp_response: 1.0,
            uv_scale: 1.0,
            albedo: 0,
            normal: 0,
            metallic_roughness: 0,
            emissive_map: 0,
            _toon_ramp: 0,
            mask: 0,
        }
    }
}

struct Mesh {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    colors: Vec<Color>,
    materials: Vec<Material>,
    indices: Vec<usize>,
    triangle_materials: Vec<usize>,
    has_normals: bool,
    has_uvs: bool,
    has_colors: bool,
    has_materials: bool,
}

#[derive(Clone)]
struct TexturePixels {
    width: usize,
    height: usize,
    _flags: u32,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
struct BakeView {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    yaw: f32,
}

#[derive(Clone, Copy, Default)]
struct ProjectedVertex {
    x: f32,
    y: f32,
    z: f32,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, len: usize, context: &str) -> Result<&'a [u8], String> {
        if self.pos + len > self.data.len() {
            return Err(format!("impostor baker: {} is truncated", context));
        }
        let out = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    fn read_u32(&mut self, context: &str) -> Result<u32, String> {
        let bytes = self.read_bytes(4, context)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_f32(&mut self, context: &str) -> Result<f32, String> {
        let bytes = self.read_bytes(4, context)?;
        Ok(f32::from_le_bytes(bytes.try_into().unwrap()))
    }
}

pub fn bake_impostor_atlas_bytes(request: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = Reader::new(request);
    let version = r.read_u32("request version")?;
    if version != 1 {
        return Err(format!(
            "impostor baker: unsupported request version {}",
            version
        ));
    }
    let width = r.read_u32("atlas width")? as usize;
    let height = r.read_u32("atlas height")? as usize;
    let view_count = r.read_u32("view count")? as usize;
    if width == 0 || height == 0 {
        return Err("impostor baker: atlas dimensions must be > 0".to_string());
    }
    if view_count == 0 {
        return Err("impostor baker: request requires at least one view".to_string());
    }
    let tint = Color {
        r: r.read_f32("base color r")?,
        g: r.read_f32("base color g")?,
        b: r.read_f32("base color b")?,
        a: r.read_f32("base color a")?,
    };
    let mut views = Vec::with_capacity(view_count);
    for _ in 0..view_count {
        views.push(BakeView {
            x: r.read_f32("view x")?,
            y: r.read_f32("view y")?,
            w: r.read_f32("view w")?,
            h: r.read_f32("view h")?,
            yaw: r.read_f32("view yaw")?,
        });
    }
    let geometry_len = r.read_u32("geometry length")? as usize;
    let geometry = r.read_bytes(geometry_len, "geometry bytes")?;
    let mesh = decode_mesh(geometry)?;
    let texture_count = r.read_u32("texture count")? as usize;
    let mut textures = HashMap::with_capacity(texture_count);
    for _ in 0..texture_count {
        let id = r.read_u32("texture id")?;
        let len = r.read_u32("texture length")? as usize;
        let payload = r.read_bytes(len, "texture payload")?;
        let texture = decode_texture_pixels(payload)?;
        textures.insert(id, texture);
    }
    if r.remaining() != 0 {
        return Err("impostor baker: request has trailing bytes".to_string());
    }

    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "impostor baker: atlas dimensions overflow".to_string())?;
    let byte_count = pixel_count
        .checked_mul(4)
        .ok_or_else(|| "impostor baker: atlas byte count overflow".to_string())?;
    let mut albedo = vec![0u8; byte_count];
    let mut normal = vec![0u8; byte_count];
    let mut metallic_roughness = vec![0u8; byte_count];
    let mut mask = vec![0u8; byte_count];
    let mut depth = vec![-1.0e30f32; pixel_count];
    for i in 0..pixel_count {
        let base = i * 4;
        normal[base] = 128;
        normal[base + 1] = 128;
        normal[base + 2] = 255;
        normal[base + 3] = 255;
        metallic_roughness[base] = 0;
        metallic_roughness[base + 1] = 190;
        metallic_roughness[base + 2] = 255;
        metallic_roughness[base + 3] = 255;
        mask[base + 3] = 255;
    }

    let bounds = mesh_bounds(&mesh)?;
    for view in views {
        let x0 = clamp_i32((view.x + 0.5) as i32, 0, width as i32) as usize;
        let y0 = clamp_i32((view.y + 0.5) as i32, 0, height as i32) as usize;
        let x1 = clamp_i32((view.x + view.w + 0.5) as i32, 0, width as i32) as usize;
        let y1 = clamp_i32((view.y + view.h + 0.5) as i32, 0, height as i32) as usize;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        raster_mesh_cell(
            &mesh,
            &textures,
            bounds,
            tint,
            view.yaw,
            x0,
            y0,
            x1,
            y1,
            width,
            &mut albedo,
            &mut normal,
            &mut metallic_roughness,
            &mut mask,
            &mut depth,
        );
    }

    encode_bake_output(width, height, &albedo, &normal, &metallic_roughness, &mask)
}

fn encode_bake_output(
    width: usize,
    height: usize,
    albedo: &[u8],
    normal: &[u8],
    metallic_roughness: &[u8],
    mask: &[u8],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(
        24 + albedo.len() + normal.len() + metallic_roughness.len() + mask.len(),
    );
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    for image in [albedo, normal, metallic_roughness, mask] {
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(image);
    }
    Ok(out)
}

fn decode_mesh(data: &[u8]) -> Result<Mesh, String> {
    let mut r = Reader::new(data);
    let version = r.read_u32("model geometry version")?;
    if version != 1 {
        return Err(format!(
            "impostor baker: unsupported model geometry version {}",
            version
        ));
    }
    let position_count = r.read_u32("position count")? as usize;
    let index_count = r.read_u32("index count")? as usize;
    let flags = r.read_u32("geometry flags")?;
    let material_count = r.read_u32("material count")? as usize;
    let triangle_material_count = r.read_u32("triangle material count")? as usize;
    if position_count == 0 || index_count < 3 {
        return Err("impostor baker: model geometry is empty".to_string());
    }
    let expected = 24usize
        .checked_add(position_count.checked_mul(48).unwrap_or(usize::MAX))
        .and_then(|v| v.checked_add(material_count.checked_mul(84).unwrap_or(usize::MAX)))
        .and_then(|v| v.checked_add(index_count.checked_mul(4).unwrap_or(usize::MAX)))
        .and_then(|v| v.checked_add(triangle_material_count.checked_mul(4).unwrap_or(usize::MAX)))
        .ok_or_else(|| "impostor baker: model geometry size overflow".to_string())?;
    if data.len() != expected {
        return Err(format!(
            "impostor baker: model geometry size mismatch: got {}, expected {}",
            data.len(),
            expected
        ));
    }
    let mut positions = Vec::with_capacity(position_count);
    let mut normals = Vec::with_capacity(position_count);
    let mut uvs = Vec::with_capacity(position_count);
    let mut colors = Vec::with_capacity(position_count);
    for _ in 0..position_count {
        positions.push(Vec3 {
            x: r.read_f32("position x")?,
            y: r.read_f32("position y")?,
            z: r.read_f32("position z")?,
        });
        normals.push(Vec3 {
            x: r.read_f32("normal x")?,
            y: r.read_f32("normal y")?,
            z: r.read_f32("normal z")?,
        });
        uvs.push(Vec2 {
            x: r.read_f32("uv x")?,
            y: r.read_f32("uv y")?,
        });
        colors.push(Color {
            r: r.read_f32("color r")?,
            g: r.read_f32("color g")?,
            b: r.read_f32("color b")?,
            a: r.read_f32("color a")?,
        });
    }
    let mut materials = Vec::with_capacity(material_count.max(1));
    for _ in 0..material_count {
        let mut material = Material {
            base_color: Color {
                r: r.read_f32("material base r")?,
                g: r.read_f32("material base g")?,
                b: r.read_f32("material base b")?,
                a: r.read_f32("material base a")?,
            },
            emissive: Vec3 {
                x: r.read_f32("material emissive r")?,
                y: r.read_f32("material emissive g")?,
                z: r.read_f32("material emissive b")?,
            },
            metallic: r.read_f32("material metallic")?,
            roughness: r.read_f32("material roughness")?,
            normal_scale: r.read_f32("material normal scale")?,
            detail_strength: r.read_f32("material detail strength")?,
            macro_blend: r.read_f32("material macro blend")?,
            roughness_response: r.read_f32("material roughness response")?,
            toon_ramp_response: r.read_f32("material toon response")?,
            uv_scale: r.read_f32("material uv scale")?,
            albedo: r.read_u32("material albedo")?,
            normal: r.read_u32("material normal")?,
            metallic_roughness: r.read_u32("material metallic roughness")?,
            emissive_map: r.read_u32("material emissive")?,
            _toon_ramp: r.read_u32("material toon ramp")?,
            mask: r.read_u32("material mask")?,
        };
        normalize_material(&mut material);
        materials.push(material);
    }
    if materials.is_empty() {
        materials.push(Material::default());
    }
    let mut indices = Vec::with_capacity(index_count);
    for _ in 0..index_count {
        let index = r.read_u32("index")? as usize;
        if index >= position_count {
            return Err(format!("impostor baker: index {} is out of range", index));
        }
        indices.push(index);
    }
    let mut triangle_materials = Vec::with_capacity(triangle_material_count);
    for _ in 0..triangle_material_count {
        let index = r.read_u32("triangle material")? as usize;
        triangle_materials.push(index.min(materials.len() - 1));
    }
    Ok(Mesh {
        positions,
        normals,
        uvs,
        colors,
        materials,
        indices,
        triangle_materials,
        has_normals: flags & 1 != 0,
        has_uvs: flags & 2 != 0,
        has_colors: flags & 4 != 0,
        has_materials: flags & 8 != 0,
    })
}

fn normalize_material(material: &mut Material) {
    if material.base_color.r == 0.0
        && material.base_color.g == 0.0
        && material.base_color.b == 0.0
        && material.base_color.a == 0.0
    {
        material.base_color = Color::default();
    }
    if material.base_color.a == 0.0 {
        material.base_color.a = 1.0;
    }
    material.metallic = material.metallic.clamp(0.0, 1.0);
    material.roughness = material.roughness.clamp(0.02, 1.0);
    if material.normal_scale <= 0.0 {
        material.normal_scale = 1.0;
    }
    if material.detail_strength <= 0.0 {
        material.detail_strength = 1.0;
    }
    if material.roughness_response <= 0.0 {
        material.roughness_response = 1.0;
    }
    if material.toon_ramp_response < 0.0 {
        material.toon_ramp_response = 0.0;
    }
    if material.uv_scale <= 0.0 {
        material.uv_scale = 1.0;
    }
}

fn decode_texture_pixels(data: &[u8]) -> Result<TexturePixels, String> {
    let mut r = Reader::new(data);
    let version = r.read_u32("texture pixels version")?;
    if version != 1 {
        return Err(format!(
            "impostor baker: unsupported texture pixels version {}",
            version
        ));
    }
    let width = r.read_u32("texture width")? as usize;
    let height = r.read_u32("texture height")? as usize;
    let flags = r.read_u32("texture flags")?;
    if width == 0 || height == 0 {
        return Err("impostor baker: texture dimensions must be > 0".to_string());
    }
    let byte_count = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(4))
        .ok_or_else(|| "impostor baker: texture byte count overflow".to_string())?;
    let pixels = r.read_bytes(byte_count, "texture pixels")?.to_vec();
    if r.remaining() != 0 {
        return Err("impostor baker: texture pixels has trailing bytes".to_string());
    }
    Ok(TexturePixels {
        width,
        height,
        _flags: flags,
        pixels,
    })
}

fn mesh_bounds(mesh: &Mesh) -> Result<(Vec3, Vec3), String> {
    let first = *mesh
        .positions
        .first()
        .ok_or_else(|| "impostor baker: model geometry is empty".to_string())?;
    let mut min = first;
    let mut max = first;
    for p in &mesh.positions {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        min.z = min.z.min(p.z);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
        max.z = max.z.max(p.z);
    }
    Ok((min, max))
}

#[allow(clippy::too_many_arguments)]
fn raster_mesh_cell(
    mesh: &Mesh,
    textures: &HashMap<u32, TexturePixels>,
    bounds: (Vec3, Vec3),
    tint: Color,
    yaw: f32,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    atlas_width: usize,
    albedo: &mut [u8],
    normal: &mut [u8],
    metallic_roughness: &mut [u8],
    mask: &mut [u8],
    depth: &mut [f32],
) {
    if mesh.positions.is_empty() || mesh.indices.len() < 3 {
        return;
    }
    let (min_bound, max_bound) = bounds;
    let center = Vec3 {
        x: (min_bound.x + max_bound.x) * 0.5,
        y: (min_bound.y + max_bound.y) * 0.5,
        z: (min_bound.z + max_bound.z) * 0.5,
    };
    let cos_yaw = yaw.cos();
    let sin_yaw = yaw.sin();
    let mut projected = vec![ProjectedVertex::default(); mesh.positions.len()];
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    let mut min_z = f32::MAX;
    let mut max_z = f32::MIN;
    for (index, pos) in mesh.positions.iter().enumerate() {
        let dx = pos.x - center.x;
        let dy = pos.y - center.y;
        let dz = pos.z - center.z;
        let vx = dx * cos_yaw - dz * sin_yaw;
        let vz = dx * sin_yaw + dz * cos_yaw;
        projected[index] = ProjectedVertex {
            x: vx,
            y: dy,
            z: vz,
        };
        min_x = min_x.min(vx);
        max_x = max_x.max(vx);
        min_y = min_y.min(dy);
        max_y = max_y.max(dy);
        min_z = min_z.min(vz);
        max_z = max_z.max(vz);
    }
    let range_x = max_x - min_x;
    let range_y = max_y - min_y;
    if range_x <= 0.000001 || range_y <= 0.000001 {
        return;
    }
    let cell_w = (x1 - x0) as f32;
    let cell_h = (y1 - y0) as f32;
    let pad = 1.0f32.max(cell_w.min(cell_h) * 0.06);
    let scale = ((cell_w - pad * 2.0) / range_x).min((cell_h - pad * 2.0) / range_y);
    if scale <= 0.0 {
        return;
    }
    let mesh_center_x = (min_x + max_x) * 0.5;
    let mesh_center_y = (min_y + max_y) * 0.5;
    let cell_center_x = (x0 + x1) as f32 * 0.5;
    let cell_center_y = (y0 + y1) as f32 * 0.5;
    let mut screen = vec![ProjectedVertex::default(); projected.len()];
    for (index, p) in projected.iter().enumerate() {
        screen[index] = ProjectedVertex {
            x: cell_center_x + (p.x - mesh_center_x) * scale,
            y: cell_center_y - (p.y - mesh_center_y) * scale,
            z: p.z,
        };
    }
    let range_z = (max_z - min_z).max(1.0e-6);
    for tri_base in (0..mesh.indices.len()).step_by(3) {
        if tri_base + 2 >= mesh.indices.len() {
            break;
        }
        let i0 = mesh.indices[tri_base];
        let i1 = mesh.indices[tri_base + 1];
        let i2 = mesh.indices[tri_base + 2];
        let p0 = screen[i0];
        let p1 = screen[i1];
        let p2 = screen[i2];
        let area = edge(p0.x, p0.y, p1.x, p1.y, p2.x, p2.y);
        if area.abs() < 0.000001 {
            continue;
        }
        let material = material_for_triangle(mesh, tri_base / 3, tint);
        let (face_nx, face_ny, face_nz) = triangle_view_normal(
            mesh.positions[i0],
            mesh.positions[i1],
            mesh.positions[i2],
            cos_yaw,
            sin_yaw,
        );
        let min_px = clamp_i32(
            p0.x.min(p1.x).min(p2.x).floor() as i32 - 1,
            x0 as i32,
            x1 as i32 - 1,
        ) as usize;
        let max_px = clamp_i32(
            p0.x.max(p1.x).max(p2.x).ceil() as i32 + 1,
            x0 as i32,
            x1 as i32 - 1,
        ) as usize;
        let min_py = clamp_i32(
            p0.y.min(p1.y).min(p2.y).floor() as i32 - 1,
            y0 as i32,
            y1 as i32 - 1,
        ) as usize;
        let max_py = clamp_i32(
            p0.y.max(p1.y).max(p2.y).ceil() as i32 + 1,
            y0 as i32,
            y1 as i32 - 1,
        ) as usize;
        for py in min_py..=max_py {
            for px in min_px..=max_px {
                let mut coverage = 0usize;
                let mut depth_sum = 0.0f32;
                for sy in 0..2 {
                    for sx in 0..2 {
                        let spx = px as f32 + 0.25 + sx as f32 * 0.5;
                        let spy = py as f32 + 0.25 + sy as f32 * 0.5;
                        let w0 = edge(p1.x, p1.y, p2.x, p2.y, spx, spy);
                        let w1 = edge(p2.x, p2.y, p0.x, p0.y, spx, spy);
                        let w2 = edge(p0.x, p0.y, p1.x, p1.y, spx, spy);
                        let inside = if area > 0.0 {
                            w0 >= -0.000001 && w1 >= -0.000001 && w2 >= -0.000001
                        } else {
                            w0 <= 0.000001 && w1 <= 0.000001 && w2 <= 0.000001
                        };
                        if inside {
                            let b0 = w0 / area;
                            let b1 = w1 / area;
                            let b2 = w2 / area;
                            depth_sum += b0 * p0.z + b1 * p1.z + b2 * p2.z;
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 {
                    continue;
                }
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                let cw0 = edge(p1.x, p1.y, p2.x, p2.y, cx, cy) / area;
                let cw1 = edge(p2.x, p2.y, p0.x, p0.y, cx, cy) / area;
                let cw2 = 1.0 - cw0 - cw1;
                let mut nx = face_nx;
                let mut ny = face_ny;
                let mut nz = face_nz;
                if mesh.has_normals {
                    (nx, ny, nz) = interpolated_view_normal(
                        mesh.normals[i0],
                        mesh.normals[i1],
                        mesh.normals[i2],
                        cw0,
                        cw1,
                        cw2,
                        cos_yaw,
                        sin_yaw,
                    );
                }
                let vertex_color = if mesh.has_colors {
                    interpolated_color(
                        mesh.colors[i0],
                        mesh.colors[i1],
                        mesh.colors[i2],
                        cw0,
                        cw1,
                        cw2,
                    )
                } else {
                    Color::default()
                };
                let mut has_uv = false;
                let mut sample_uv = Vec2::default();
                let mut uv_noise = 0.0f32;
                if mesh.has_uvs {
                    let uv =
                        interpolated_uv(mesh.uvs[i0], mesh.uvs[i1], mesh.uvs[i2], cw0, cw1, cw2);
                    sample_uv = Vec2 {
                        x: uv.x * material.uv_scale,
                        y: uv.y * material.uv_scale,
                    };
                    has_uv = true;
                    uv_noise = (sample_uv.x * 43.0 + sample_uv.y * 37.0).sin()
                        * 0.025
                        * material.detail_strength;
                }
                let albedo_sample = sample_or(
                    textures,
                    material.albedo,
                    sample_uv,
                    Color::default(),
                    has_uv,
                );
                let normal_sample = sample_optional(textures, material.normal, sample_uv, has_uv);
                let mr_sample =
                    sample_optional(textures, material.metallic_roughness, sample_uv, has_uv);
                let emissive_sample = sample_or(
                    textures,
                    material.emissive_map,
                    sample_uv,
                    Color::default(),
                    has_uv,
                );
                let mask_sample =
                    sample_or(textures, material.mask, sample_uv, Color::default(), has_uv);
                let pixel = py * atlas_width + px;
                let z = depth_sum / coverage as f32;
                if z < depth[pixel] {
                    continue;
                }
                depth[pixel] = z;
                let mut alpha = coverage as f32 * 0.25;
                let normal_scale = material.normal_scale.max(0.0);
                if let Some(sample) = normal_sample {
                    nx += (sample.r * 2.0 - 1.0) * 0.55 * normal_scale;
                    ny += (sample.g * 2.0 - 1.0) * 0.55 * normal_scale;
                    nz += ((sample.b * 2.0 - 1.0) - 1.0) * 0.35 * normal_scale;
                    (nx, ny, nz) = normalize3(nx, ny, nz, (0.0, 0.0, 1.0));
                } else if (normal_scale - 1.0).abs() > f32::EPSILON {
                    nx *= normal_scale;
                    ny *= normal_scale;
                    (nx, ny, nz) = normalize3(nx, ny, nz, (0.0, 0.0, 1.0));
                }
                alpha *= albedo_sample.a * mask_sample.a;
                if alpha <= 0.0 {
                    continue;
                }
                let depth_shade = 0.88 + 0.16 * ((z - min_z) / range_z);
                let height_shade =
                    0.88 + 0.18 * (1.0 - ((py - y0) as f32 / (y1 - y0).max(1) as f32));
                let ndotl = (nx * -0.34 + ny * 0.72 + nz * 0.60).max(0.0);
                let rim = 1.0 - nz.abs().min(1.0);
                let macro_noise = ((px as f32 * 0.071 + yaw * 3.1).sin()
                    * (py as f32 * 0.053 + tri_base as f32 * 0.017).cos())
                    * material.macro_blend
                    * 0.045;
                let micro = 0.96
                    + uv_noise
                    + macro_noise
                    + 0.04 * (px as f32 * 0.47 + py as f32 * 0.31 + tri_base as f32 * 0.09).sin();
                let mut shade =
                    ((0.42 + 0.48 * ndotl + 0.08 * rim) * depth_shade * height_shade * micro)
                        .clamp(0.22, 1.32);
                if material.toon_ramp_response > 0.0 {
                    let steps = (5.0 - material.toon_ramp_response * 2.0).max(2.0);
                    let quantized = (shade * steps + 0.5).floor() / steps;
                    let response = (material.toon_ramp_response * 0.28).clamp(0.0, 0.56);
                    shade = shade * (1.0 - response) + quantized * response;
                }
                let out = pixel * 4;
                let emissive = Vec3 {
                    x: material.emissive.x * emissive_sample.r,
                    y: material.emissive.y * emissive_sample.g,
                    z: material.emissive.z * emissive_sample.b,
                };
                albedo[out] = color_byte(
                    material.base_color.r * albedo_sample.r * vertex_color.r * shade + emissive.x,
                );
                albedo[out + 1] = color_byte(
                    material.base_color.g * albedo_sample.g * vertex_color.g * shade + emissive.y,
                );
                albedo[out + 2] = color_byte(
                    material.base_color.b * albedo_sample.b * vertex_color.b * shade + emissive.z,
                );
                albedo[out + 3] = color_byte(material.base_color.a * vertex_color.a * alpha);
                normal[out] = color_byte(nx * 0.5 + 0.5);
                normal[out + 1] = color_byte(ny * 0.5 + 0.5);
                normal[out + 2] = color_byte(nz * 0.5 + 0.5);
                normal[out + 3] = 255;
                let response = material.roughness_response.clamp(0.0, 4.0);
                let mut roughness_base =
                    material.roughness * (1.0 + (mask_sample.r - 1.0) * response);
                let mut metallic_base =
                    material.metallic * (1.0 + (mask_sample.g - 1.0) * response);
                if let Some(sample) = mr_sample {
                    roughness_base *= 1.0 + (sample.g - 1.0) * response;
                    metallic_base *= 1.0 + (sample.b - 1.0) * response;
                }
                let roughness = (roughness_base
                    + (0.24 * (1.0 - ndotl) + 0.10 * rim) * material.roughness_response)
                    .clamp(0.02, 1.0);
                metallic_roughness[out] = color_byte(metallic_base.clamp(0.0, 1.0));
                metallic_roughness[out + 1] = color_byte(roughness);
                metallic_roughness[out + 2] = 255;
                metallic_roughness[out + 3] = 255;
                mask[out] = color_byte(alpha);
                mask[out + 1] = color_byte(mask_sample.r);
                mask[out + 2] = color_byte(mask_sample.g);
                mask[out + 3] = 255;
            }
        }
    }
}

fn material_for_triangle(mesh: &Mesh, triangle: usize, tint: Color) -> Material {
    let mut material = if mesh.has_materials && triangle < mesh.triangle_materials.len() {
        let index = mesh.triangle_materials[triangle].min(mesh.materials.len() - 1);
        mesh.materials[index]
    } else {
        Material::default()
    };
    material.base_color.r *= tint.r;
    material.base_color.g *= tint.g;
    material.base_color.b *= tint.b;
    material.base_color.a *= tint.a;
    normalize_material(&mut material);
    material
}

fn sample_or(
    textures: &HashMap<u32, TexturePixels>,
    id: u32,
    uv: Vec2,
    fallback: Color,
    enabled: bool,
) -> Color {
    sample_optional(textures, id, uv, enabled).unwrap_or(fallback)
}

fn sample_optional(
    textures: &HashMap<u32, TexturePixels>,
    id: u32,
    uv: Vec2,
    enabled: bool,
) -> Option<Color> {
    if !enabled || id == 0 {
        return None;
    }
    textures.get(&id).map(|texture| texture.sample(uv))
}

impl TexturePixels {
    fn sample(&self, uv: Vec2) -> Color {
        let u = wrap01(uv.x);
        let v = wrap01(uv.y);
        let fx = u * (self.width.saturating_sub(1)) as f32;
        let fy = v * (self.height.saturating_sub(1)) as f32;
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let c00 = self.texel(x0, y0);
        let c10 = self.texel(x1, y0);
        let c01 = self.texel(x0, y1);
        let c11 = self.texel(x1, y1);
        lerp_color(lerp_color(c00, c10, tx), lerp_color(c01, c11, tx), ty)
    }

    fn texel(&self, x: usize, y: usize) -> Color {
        let i = (y * self.width + x) * 4;
        Color {
            r: self.pixels[i] as f32 / 255.0,
            g: self.pixels[i + 1] as f32 / 255.0,
            b: self.pixels[i + 2] as f32 / 255.0,
            a: self.pixels[i + 3] as f32 / 255.0,
        }
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn triangle_view_normal(a: Vec3, b: Vec3, c: Vec3, cos_yaw: f32, sin_yaw: f32) -> (f32, f32, f32) {
    let ux = b.x - a.x;
    let uy = b.y - a.y;
    let uz = b.z - a.z;
    let vx = c.x - a.x;
    let vy = c.y - a.y;
    let vz = c.z - a.z;
    let nx = uy * vz - uz * vy;
    let ny = uz * vx - ux * vz;
    let nz = ux * vy - uy * vx;
    let (nx, ny, nz) = normalize3(nx, ny, nz, (0.0, 0.0, 1.0));
    view_normal(nx, ny, nz, cos_yaw, sin_yaw)
}

fn interpolated_view_normal(
    a: Vec3,
    b: Vec3,
    c: Vec3,
    wa: f32,
    wb: f32,
    wc: f32,
    cos_yaw: f32,
    sin_yaw: f32,
) -> (f32, f32, f32) {
    let nx = a.x * wa + b.x * wb + c.x * wc;
    let ny = a.y * wa + b.y * wb + c.y * wc;
    let nz = a.z * wa + b.z * wb + c.z * wc;
    let (nx, ny, nz) = normalize3(nx, ny, nz, (0.0, 0.0, 1.0));
    view_normal(nx, ny, nz, cos_yaw, sin_yaw)
}

fn view_normal(nx: f32, ny: f32, nz: f32, cos_yaw: f32, sin_yaw: f32) -> (f32, f32, f32) {
    let mut view_x = nx * cos_yaw - nz * sin_yaw;
    let mut view_y = ny;
    let mut view_z = nx * sin_yaw + nz * cos_yaw;
    if view_z < 0.0 {
        view_x = -view_x;
        view_y = -view_y;
        view_z = -view_z;
    }
    view_z = view_z.max(0.001);
    normalize3(view_x, view_y, view_z, (0.0, 0.0, 1.0))
}

fn normalize3(x: f32, y: f32, z: f32, fallback: (f32, f32, f32)) -> (f32, f32, f32) {
    let len = (x * x + y * y + z * z).sqrt();
    if len <= 0.000001 {
        fallback
    } else {
        (x / len, y / len, z / len)
    }
}

fn interpolated_color(a: Color, b: Color, c: Color, wa: f32, wb: f32, wc: f32) -> Color {
    Color {
        r: a.r * wa + b.r * wb + c.r * wc,
        g: a.g * wa + b.g * wb + c.g * wc,
        b: a.b * wa + b.b * wb + c.b * wc,
        a: a.a * wa + b.a * wb + c.a * wc,
    }
}

fn interpolated_uv(a: Vec2, b: Vec2, c: Vec2, wa: f32, wb: f32, wc: f32) -> Vec2 {
    Vec2 {
        x: a.x * wa + b.x * wb + c.x * wc,
        y: a.y * wa + b.y * wb + c.y * wc,
    }
}

fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

fn wrap01(v: f32) -> f32 {
    let wrapped = v - v.floor();
    if wrapped < 0.0 {
        wrapped + 1.0
    } else {
        wrapped
    }
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    v.max(lo).min(hi)
}

fn color_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_v1_request() {
        let mut data = Vec::new();
        data.extend_from_slice(&2u32.to_le_bytes());
        let error = bake_impostor_atlas_bytes(&data).unwrap_err();
        assert!(error.contains("unsupported request version"));
    }
}
