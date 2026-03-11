use std::f32::consts::{PI, TAU};

use crate::model_loader::MeshVertex;

fn vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> MeshVertex {
    MeshVertex { position, normal, uv }
}

pub fn generate_plane(width: f32, depth: f32, sub_x: u32, sub_z: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(sub_x >= 1, "generate_plane: sub_x must be >= 1");
    assert!(sub_z >= 1, "generate_plane: sub_z must be >= 1");

    let mut vertices = Vec::with_capacity(((sub_x + 1) * (sub_z + 1)) as usize);
    let mut indices = Vec::with_capacity((sub_x * sub_z * 6) as usize);

    for iz in 0..=sub_z {
        let vz = iz as f32 / sub_z as f32;
        let z = (vz - 0.5) * depth;
        for ix in 0..=sub_x {
            let ux = ix as f32 / sub_x as f32;
            let x = (ux - 0.5) * width;
            vertices.push(vertex([x, 0.0, z], [0.0, 1.0, 0.0], [ux, vz]));
        }
    }

    let stride = sub_x + 1;
    for iz in 0..sub_z {
        for ix in 0..sub_x {
            let i0 = iz * stride + ix;
            let i1 = (iz + 1) * stride + ix;
            let i2 = i0 + 1;
            let i3 = i1 + 1;
            indices.extend_from_slice(&[i0, i1, i2, i2, i1, i3]);
        }
    }

    (vertices, indices)
}

pub fn generate_cube() -> (Vec<MeshVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [1.0, 0.0, 0.0],
            [
                [0.5, 0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, -0.5],
                [0.5, -0.5, -0.5],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-0.5, 0.5, -0.5],
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, -0.5, 0.5],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [-0.5, 0.5, -0.5],
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-0.5, -0.5, 0.5],
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [0.5, -0.5, -0.5],
            ],
        ),
        (
            [0.0, 0.0, 1.0],
            [
                [-0.5, 0.5, 0.5],
                [-0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, -0.5, 0.5],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [0.5, 0.5, -0.5],
                [0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [-0.5, -0.5, -0.5],
            ],
        ),
    ];
    let uvs = [[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]];

    for (face_index, (normal, positions)) in faces.into_iter().enumerate() {
        let base = (face_index * 4) as u32;
        for i in 0..4 {
            vertices.push(vertex(positions[i], normal, uvs[i]));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }

    (vertices, indices)
}

pub fn generate_sphere(segments: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(segments >= 3, "generate_sphere: segments must be >= 3");

    let lat_segments = segments;
    let lon_segments = segments * 2;
    let mut vertices = Vec::with_capacity(((lat_segments + 1) * (lon_segments + 1)) as usize);
    let mut indices = Vec::with_capacity((lat_segments * lon_segments * 6) as usize);

    for iy in 0..=lat_segments {
        let v = iy as f32 / lat_segments as f32;
        let theta = v * PI;
        let sin_theta = theta.sin();
        let cos_theta = theta.cos();
        let y = cos_theta * 0.5;
        let ring_radius = sin_theta * 0.5;

        for ix in 0..=lon_segments {
            let u = ix as f32 / lon_segments as f32;
            let phi = u * TAU;
            let cos_phi = phi.cos();
            let sin_phi = phi.sin();
            vertices.push(vertex(
                [ring_radius * cos_phi, y, ring_radius * sin_phi],
                [sin_theta * cos_phi, cos_theta, sin_theta * sin_phi],
                [u, v],
            ));
        }
    }

    let stride = lon_segments + 1;
    for iy in 0..lat_segments {
        for ix in 0..lon_segments {
            let i0 = iy * stride + ix;
            let i1 = i0 + stride;
            if iy != 0 {
                indices.extend_from_slice(&[i0, i0 + 1, i1]);
            }
            if iy != lat_segments - 1 {
                indices.extend_from_slice(&[i0 + 1, i1 + 1, i1]);
            }
        }
    }

    (vertices, indices)
}

pub fn generate_cylinder(segments: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(segments >= 3, "generate_cylinder: segments must be >= 3");

    let radius = 0.5;
    let half_height = 0.5;
    let ring = segments + 1;
    let mut vertices = Vec::with_capacity((ring * 4 + 2) as usize);
    let mut indices = Vec::with_capacity((segments * 12) as usize);

    for iy in 0..=1 {
        let y = if iy == 0 { half_height } else { -half_height };
        let v = iy as f32;
        for ix in 0..=segments {
            let u = ix as f32 / segments as f32;
            let phi = u * TAU;
            let cos_phi = phi.cos();
            let sin_phi = phi.sin();
            vertices.push(vertex(
                [radius * cos_phi, y, radius * sin_phi],
                [cos_phi, 0.0, sin_phi],
                [u, v],
            ));
        }
    }

    for ix in 0..segments {
        let i0 = ix;
        let i1 = i0 + 1;
        let i2 = i0 + ring;
        let i3 = i2 + 1;
        indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
    }

    let top_center = vertices.len() as u32;
    vertices.push(vertex([0.0, half_height, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5]));
    let top_ring_start = vertices.len() as u32;
    for ix in 0..=segments {
        let u = ix as f32 / segments as f32;
        let phi = u * TAU;
        let cos_phi = phi.cos();
        let sin_phi = phi.sin();
        vertices.push(vertex(
            [radius * cos_phi, half_height, radius * sin_phi],
            [0.0, 1.0, 0.0],
            [0.5 + cos_phi * 0.5, 0.5 + sin_phi * 0.5],
        ));
    }
    for ix in 0..segments {
        let current = top_ring_start + ix;
        let next = current + 1;
        indices.extend_from_slice(&[top_center, next, current]);
    }

    let bottom_center = vertices.len() as u32;
    vertices.push(vertex([0.0, -half_height, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]));
    let bottom_ring_start = vertices.len() as u32;
    for ix in 0..=segments {
        let u = ix as f32 / segments as f32;
        let phi = u * TAU;
        let cos_phi = phi.cos();
        let sin_phi = phi.sin();
        vertices.push(vertex(
            [radius * cos_phi, -half_height, radius * sin_phi],
            [0.0, -1.0, 0.0],
            [0.5 + cos_phi * 0.5, 0.5 + sin_phi * 0.5],
        ));
    }
    for ix in 0..segments {
        let current = bottom_ring_start + ix;
        let next = current + 1;
        indices.extend_from_slice(&[bottom_center, current, next]);
    }

    (vertices, indices)
}

pub fn generate_capsule(segments: u32, half_height: f32, radius: f32) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(segments >= 3, "generate_capsule: segments must be >= 3");
    assert!(half_height >= 0.0, "generate_capsule: half_height must be >= 0");
    assert!(radius > 0.0, "generate_capsule: radius must be > 0");

    let lon_segments = segments * 2;
    let stride = lon_segments + 1;
    let top_extent = half_height + radius;
    let min_y = -top_extent;
    let max_y = top_extent;

    let mut rings: Vec<(f32, f32, f32, f32)> = Vec::with_capacity((segments * 2 + 2) as usize);
    rings.push((half_height + radius, 0.0, 1.0, 0.0));
    for i in 1..segments {
        let theta = i as f32 / segments as f32 * (PI * 0.5);
        rings.push((
            half_height + theta.cos() * radius,
            theta.sin() * radius,
            theta.cos(),
            theta.sin(),
        ));
    }
    rings.push((half_height, radius, 0.0, 1.0));
    rings.push((-half_height, radius, 0.0, 1.0));
    for i in 1..segments {
        let theta = i as f32 / segments as f32 * (PI * 0.5);
        rings.push((
            -half_height - theta.sin() * radius,
            theta.cos() * radius,
            -theta.sin(),
            theta.cos(),
        ));
    }
    rings.push((-half_height - radius, 0.0, -1.0, 0.0));

    let mut vertices = Vec::with_capacity((rings.len() as u32 * stride) as usize);
    let mut indices = Vec::with_capacity(((rings.len() as u32 - 1) * lon_segments * 6) as usize);

    for &(y, ring_radius, normal_y, normal_radius) in &rings {
        let v = 1.0 - (y - min_y) / (max_y - min_y);
        for ix in 0..=lon_segments {
            let u = ix as f32 / lon_segments as f32;
            let phi = u * TAU;
            let cos_phi = phi.cos();
            let sin_phi = phi.sin();
            vertices.push(vertex(
                [ring_radius * cos_phi, y, ring_radius * sin_phi],
                [normal_radius * cos_phi, normal_y, normal_radius * sin_phi],
                [u, v],
            ));
        }
    }

    let ring_count = rings.len() as u32;
    for iy in 0..(ring_count - 1) {
        for ix in 0..lon_segments {
            let i0 = iy * stride + ix;
            let i1 = i0 + stride;
            if iy != 0 {
                indices.extend_from_slice(&[i0, i0 + 1, i1]);
            }
            if iy != ring_count - 2 {
                indices.extend_from_slice(&[i0 + 1, i1 + 1, i1]);
            }
        }
    }

    (vertices, indices)
}
