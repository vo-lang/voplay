use std::f32::consts::{PI, TAU};

use crate::model_loader::MeshVertex;

fn fallback_tangent(normal: [f32; 3]) -> [f32; 4] {
    let reference = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let dot = reference[0] * normal[0] + reference[1] * normal[1] + reference[2] * normal[2];
    let mut tangent = [
        reference[0] - normal[0] * dot,
        reference[1] - normal[1] * dot,
        reference[2] - normal[2] * dot,
    ];
    let len = (tangent[0] * tangent[0] + tangent[1] * tangent[1] + tangent[2] * tangent[2]).sqrt();
    if len > 0.000001 {
        tangent[0] /= len;
        tangent[1] /= len;
        tangent[2] /= len;
    }
    [tangent[0], tangent[1], tangent[2], 1.0]
}

fn vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> MeshVertex {
    MeshVertex {
        position,
        normal,
        uv,
        tangent: fallback_tangent(normal),
        color: [1.0, 1.0, 1.0, 1.0],
    }
}

pub fn generate_plane(
    width: f32,
    depth: f32,
    sub_x: u32,
    sub_z: u32,
) -> (Vec<MeshVertex>, Vec<u32>) {
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
    vertices.push(vertex(
        [0.0, -half_height, 0.0],
        [0.0, -1.0, 0.0],
        [0.5, 0.5],
    ));
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

pub fn generate_capsule(
    segments: u32,
    half_height: f32,
    radius: f32,
) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(segments >= 3, "generate_capsule: segments must be >= 3");
    assert!(
        half_height >= 0.0,
        "generate_capsule: half_height must be >= 0"
    );
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

pub fn generate_cone(segments: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    assert!(segments >= 3, "generate_cone: segments must be >= 3");
    let radius = 0.5;
    let half_height = 0.5;
    let mut vertices = Vec::with_capacity((segments * 4 + 1) as usize);
    let mut indices = Vec::with_capacity((segments * 6) as usize);

    let side_start = vertices.len() as u32;
    for ix in 0..segments {
        let u0 = ix as f32 / segments as f32;
        let u1 = (ix + 1) as f32 / segments as f32;
        let p0 = u0 * TAU;
        let p1 = u1 * TAU;
        let c0 = p0.cos();
        let s0 = p0.sin();
        let c1 = p1.cos();
        let s1 = p1.sin();
        let mid = (u0 + u1) * 0.5 * TAU;
        let normal = [mid.cos() * 0.8944272, 0.4472136, mid.sin() * 0.8944272];
        let base = side_start + ix * 3;
        vertices.push(vertex(
            [radius * c0, -half_height, radius * s0],
            normal,
            [u0, 1.0],
        ));
        vertices.push(vertex(
            [radius * c1, -half_height, radius * s1],
            normal,
            [u1, 1.0],
        ));
        vertices.push(vertex(
            [0.0, half_height, 0.0],
            normal,
            [(u0 + u1) * 0.5, 0.0],
        ));
        indices.extend_from_slice(&[base, base + 2, base + 1]);
    }

    let bottom_center = vertices.len() as u32;
    vertices.push(vertex(
        [0.0, -half_height, 0.0],
        [0.0, -1.0, 0.0],
        [0.5, 0.5],
    ));
    let bottom_start = vertices.len() as u32;
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
        indices.extend_from_slice(&[bottom_center, bottom_start + ix, bottom_start + ix + 1]);
    }

    (vertices, indices)
}

pub fn generate_wedge() -> (Vec<MeshVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut add_face = |positions: &[[f32; 3]], normal: [f32; 3], uvs: &[[f32; 2]]| {
        let base = vertices.len() as u32;
        for i in 0..positions.len() {
            vertices.push(vertex(positions[i], normal, uvs[i]));
        }
        if positions.len() == 3 {
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        } else {
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    };

    let a = [-0.5, -0.5, -0.5];
    let b = [0.5, -0.5, -0.5];
    let c = [-0.5, -0.5, 0.5];
    let d = [0.5, -0.5, 0.5];
    let e = [-0.5, 0.5, 0.5];
    let f = [0.5, 0.5, 0.5];
    let quad_uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let tri_uv = [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];

    add_face(&[a, b, d, c], [0.0, -1.0, 0.0], &quad_uv);
    add_face(&[c, d, f, e], [0.0, 0.0, 1.0], &quad_uv);
    add_face(&[a, c, e], [-1.0, 0.0, 0.0], &tri_uv);
    add_face(&[b, f, d], [1.0, 0.0, 0.0], &tri_uv);
    let ramp_normal = [0.0, 0.70710677, -0.70710677];
    add_face(&[a, e, f, b], ramp_normal, &quad_uv);

    (vertices, indices)
}

pub fn generate_rounded_box(bevel_radius: f32, segments: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    let radius = bevel_radius.clamp(0.0, 0.49);
    if radius <= 0.0001 {
        return generate_cube();
    }
    let grid = segments.max(2);
    let mut vertices = Vec::with_capacity((6 * (grid + 1) * (grid + 1)) as usize);
    let mut indices = Vec::with_capacity((6 * grid * grid * 6) as usize);

    let mut add_face = |axis: usize, sign: f32| {
        let base = vertices.len() as u32;
        for iy in 0..=grid {
            let v = iy as f32 / grid as f32;
            for ix in 0..=grid {
                let u = ix as f32 / grid as f32;
                let a = (u - 0.5) * 1.0;
                let b = (v - 0.5) * 1.0;
                let mut p = [0.0f32; 3];
                p[axis] = 0.5 * sign;
                p[(axis + 1) % 3] = a;
                p[(axis + 2) % 3] = b;
                let core = [
                    p[0].clamp(-0.5 + radius, 0.5 - radius),
                    p[1].clamp(-0.5 + radius, 0.5 - radius),
                    p[2].clamp(-0.5 + radius, 0.5 - radius),
                ];
                let delta = [p[0] - core[0], p[1] - core[1], p[2] - core[2]];
                let len = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                let normal = if len > 0.000001 {
                    [delta[0] / len, delta[1] / len, delta[2] / len]
                } else {
                    let mut n = [0.0f32; 3];
                    n[axis] = sign;
                    n
                };
                vertices.push(vertex(
                    [
                        core[0] + normal[0] * radius,
                        core[1] + normal[1] * radius,
                        core[2] + normal[2] * radius,
                    ],
                    normal,
                    [u, v],
                ));
            }
        }
        let stride = grid + 1;
        for iy in 0..grid {
            for ix in 0..grid {
                let i0 = base + iy * stride + ix;
                let i1 = i0 + 1;
                let i2 = i0 + stride;
                let i3 = i2 + 1;
                if sign > 0.0 {
                    indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
                } else {
                    indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                }
            }
        }
    };

    add_face(0, 1.0);
    add_face(0, -1.0);
    add_face(1, 1.0);
    add_face(1, -1.0);
    add_face(2, 1.0);
    add_face(2, -1.0);

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::{generate_cone, generate_cube, generate_rounded_box, generate_wedge};
    use crate::model_loader::MeshVertex;

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let len = dot(v, v).sqrt();
        if len <= 0.000001 {
            return [0.0, 0.0, 0.0];
        }
        [v[0] / len, v[1] / len, v[2] / len]
    }

    fn assert_triangles_match_vertex_normals(vertices: &[MeshVertex], indices: &[u32]) {
        for tri in indices.chunks_exact(3) {
            let a = &vertices[tri[0] as usize];
            let b = &vertices[tri[1] as usize];
            let c = &vertices[tri[2] as usize];
            let geometric_normal = normalize(cross(
                sub(b.position, a.position),
                sub(c.position, a.position),
            ));
            let vertex_normal = normalize([
                (a.normal[0] + b.normal[0] + c.normal[0]) / 3.0,
                (a.normal[1] + b.normal[1] + c.normal[1]) / 3.0,
                (a.normal[2] + b.normal[2] + c.normal[2]) / 3.0,
            ]);
            assert!(
                dot(geometric_normal, vertex_normal) > 0.0,
                "triangle winding must agree with vertex normals"
            );
        }
    }

    #[test]
    fn production_primitives_generate_mesh_data() {
        let (cone_vertices, cone_indices) = generate_cone(12);
        assert!(!cone_vertices.is_empty());
        assert!(!cone_indices.is_empty());
        assert_eq!(cone_indices.len() % 3, 0);

        let (wedge_vertices, wedge_indices) = generate_wedge();
        assert!(!wedge_vertices.is_empty());
        assert!(!wedge_indices.is_empty());
        assert_eq!(wedge_indices.len() % 3, 0);

        let (box_vertices, box_indices) = generate_rounded_box(0.08, 6);
        assert!(box_vertices.len() > 24);
        assert!(!box_indices.is_empty());
        assert_eq!(box_indices.len() % 3, 0);
    }

    #[test]
    fn production_primitives_are_wound_outward() {
        let (cone_vertices, cone_indices) = generate_cone(12);
        assert_triangles_match_vertex_normals(&cone_vertices, &cone_indices);

        let (cube_vertices, cube_indices) = generate_cube();
        assert_triangles_match_vertex_normals(&cube_vertices, &cube_indices);

        let (box_vertices, box_indices) = generate_rounded_box(0.08, 6);
        assert_triangles_match_vertex_normals(&box_vertices, &box_indices);
    }
}
