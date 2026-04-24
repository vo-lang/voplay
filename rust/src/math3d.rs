//! GPU-side math utilities for 3D and 2D rendering.
//!
//! All matrices are in **column-major** layout (wgpu convention).
//! Provides: Vec3, Quat, Mat4 types and construction helpers for
//! model/view/projection transforms.

use std::ops::{Add, Mul, Neg, Sub};

// ─── Vec3 ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    pub const UP: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    pub fn cross(self, rhs: Self) -> Self {
        Self {
            x: self.y * rhs.z - self.z * rhs.y,
            y: self.z * rhs.x - self.x * rhs.z,
            z: self.x * rhs.y - self.y * rhs.x,
        }
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn normalize(self) -> Self {
        let l = self.length();
        if l == 0.0 {
            return Self::ZERO;
        }
        self * (1.0 / l)
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn from_array(a: [f32; 3]) -> Self {
        Self {
            x: a[0],
            y: a[1],
            z: a[2],
        }
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

// ─── Quat ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    pub fn normalize(self) -> Self {
        let len = self.length();
        if len <= 1e-8 {
            return Self::IDENTITY;
        }
        Self {
            x: self.x / len,
            y: self.y / len,
            z: self.z / len,
            w: self.w / len,
        }
    }
}

// ─── Mat4 ──────────────────────────────────────────────────────────────────

pub type Mat4 = [[f32; 4]; 4];

pub const MAT4_IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

// ─── Mat4 operations ───────────────────────────────────────────────────────

pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    out
}

pub fn mat4_mul_vec4(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0] * v[3],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1] * v[3],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2] * v[3],
        m[0][3] * v[0] + m[1][3] * v[1] + m[2][3] * v[2] + m[3][3] * v[3],
    ]
}

pub fn transpose_upper3x3(m: &Mat4) -> Mat4 {
    [
        [m[0][0], m[1][0], m[2][0], 0.0],
        [m[0][1], m[1][1], m[2][1], 0.0],
        [m[0][2], m[1][2], m[2][2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn mat4_inverse(m: &Mat4) -> Option<Mat4> {
    let mut aug = [[0.0f32; 8]; 4];
    for row in 0..4 {
        for col in 0..4 {
            aug[row][col] = m[col][row];
        }
        aug[row][4 + row] = 1.0;
    }

    for pivot in 0..4 {
        let mut best_row = pivot;
        let mut best_abs = aug[pivot][pivot].abs();
        for row in (pivot + 1)..4 {
            let value = aug[row][pivot].abs();
            if value > best_abs {
                best_abs = value;
                best_row = row;
            }
        }
        if best_abs <= 1e-8 {
            return None;
        }
        if best_row != pivot {
            aug.swap(pivot, best_row);
        }

        let pivot_value = aug[pivot][pivot];
        for col in 0..8 {
            aug[pivot][col] /= pivot_value;
        }

        for row in 0..4 {
            if row == pivot {
                continue;
            }
            let factor = aug[row][pivot];
            if factor == 0.0 {
                continue;
            }
            for col in 0..8 {
                aug[row][col] -= factor * aug[pivot][col];
            }
        }
    }

    let mut inv = [[0.0f32; 4]; 4];
    for row in 0..4 {
        for col in 0..4 {
            inv[col][row] = aug[row][4 + col];
        }
    }
    Some(inv)
}

fn quat_from_basis(col0: Vec3, col1: Vec3, col2: Vec3) -> Quat {
    let m00 = col0.x;
    let m01 = col1.x;
    let m02 = col2.x;
    let m10 = col0.y;
    let m11 = col1.y;
    let m12 = col2.y;
    let m20 = col0.z;
    let m21 = col1.z;
    let m22 = col2.z;
    let trace = m00 + m11 + m22;
    let quat = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat::new((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        Quat::new(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        Quat::new((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        Quat::new((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    quat.normalize()
}

pub fn decompose_matrix(m: &Mat4) -> Option<(Vec3, Quat, Vec3)> {
    let translation = Vec3::new(m[3][0], m[3][1], m[3][2]);
    let mut col0 = Vec3::new(m[0][0], m[0][1], m[0][2]);
    let mut col1 = Vec3::new(m[1][0], m[1][1], m[1][2]);
    let mut col2 = Vec3::new(m[2][0], m[2][1], m[2][2]);
    let mut scale = Vec3::new(col0.length(), col1.length(), col2.length());
    if scale.x <= 1e-8 || scale.y <= 1e-8 || scale.z <= 1e-8 {
        return None;
    }
    col0 = col0 * (1.0 / scale.x);
    col1 = col1 * (1.0 / scale.y);
    col2 = col2 * (1.0 / scale.z);
    if col0.dot(col1).abs() > 1e-3 || col0.dot(col2).abs() > 1e-3 || col1.dot(col2).abs() > 1e-3 {
        return None;
    }
    let determinant = col0.dot(col1.cross(col2));
    if determinant.abs() <= 1e-6 {
        return None;
    }
    if determinant < 0.0 {
        scale.x = -scale.x;
        col0 = -col0;
    }
    let rotation = quat_from_basis(col0, col1, col2);
    Some((translation, rotation, scale))
}

// ─── Model / View / Projection ─────────────────────────────────────────────

pub fn model_matrix(pos: Vec3, rot: Quat, scale: Vec3) -> Mat4 {
    let x2 = rot.x + rot.x;
    let y2 = rot.y + rot.y;
    let z2 = rot.z + rot.z;
    let xx = rot.x * x2;
    let xy = rot.x * y2;
    let xz = rot.x * z2;
    let yy = rot.y * y2;
    let yz = rot.y * z2;
    let zz = rot.z * z2;
    let wx = rot.w * x2;
    let wy = rot.w * y2;
    let wz = rot.w * z2;

    [
        [
            (1.0 - (yy + zz)) * scale.x,
            (xy + wz) * scale.x,
            (xz - wy) * scale.x,
            0.0,
        ],
        [
            (xy - wz) * scale.y,
            (1.0 - (xx + zz)) * scale.y,
            (yz + wx) * scale.y,
            0.0,
        ],
        [
            (xz + wy) * scale.z,
            (yz - wx) * scale.z,
            (1.0 - (xx + yy)) * scale.z,
            0.0,
        ],
        [pos.x, pos.y, pos.z, 1.0],
    ]
}

pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);

    [
        [s.x, u.x, -f.x, 0.0],
        [s.y, u.y, -f.y, 0.0],
        [s.z, u.z, -f.z, 0.0],
        [-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0],
    ]
}

pub fn view_rotation_only(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let mut view = look_at_rh(eye, target, up);
    view[3][0] = 0.0;
    view[3][1] = 0.0;
    view[3][2] = 0.0;
    view
}

pub fn perspective_rh_zo(fov_y_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let h = 1.0 / (fov_y_rad * 0.5).tan();
    let w = h / aspect;
    let r = far / (near - far);

    [
        [w, 0.0, 0.0, 0.0],
        [0.0, h, 0.0, 0.0],
        [0.0, 0.0, r, -1.0],
        [0.0, 0.0, r * near, 0.0],
    ]
}

pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let w = right - left;
    let h = top - bottom;
    let d = far - near;
    [
        [2.0 / w, 0.0, 0.0, 0.0],
        [0.0, 2.0 / h, 0.0, 0.0],
        [0.0, 0.0, -2.0 / d, 0.0],
        [
            -(right + left) / w,
            -(top + bottom) / h,
            -(far + near) / d,
            1.0,
        ],
    ]
}

pub fn orthographic_rh_zo(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> Mat4 {
    let w = right - left;
    let h = top - bottom;
    let d = near - far;
    [
        [2.0 / w, 0.0, 0.0, 0.0],
        [0.0, 2.0 / h, 0.0, 0.0],
        [0.0, 0.0, 1.0 / d, 0.0],
        [
            (left + right) / (left - right),
            (top + bottom) / (bottom - top),
            near / d,
            1.0,
        ],
    ]
}

pub fn compute_shadow_vp(camera_inv_vp: &Mat4, light_dir: Vec3) -> Mat4 {
    let ndc_corners = [
        [-1.0, -1.0, 0.0, 1.0],
        [-1.0, 1.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0, 1.0],
        [-1.0, -1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0, 1.0],
        [1.0, -1.0, 1.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
    ];

    let mut world_corners = [Vec3::ZERO; 8];
    let mut center = Vec3::ZERO;
    for (index, ndc) in ndc_corners.iter().enumerate() {
        let corner = mat4_mul_vec4(camera_inv_vp, *ndc);
        let inv_w = 1.0 / corner[3];
        let world = Vec3::new(corner[0] * inv_w, corner[1] * inv_w, corner[2] * inv_w);
        world_corners[index] = world;
        center = center + world;
    }
    center = center * (1.0 / world_corners.len() as f32);

    let mut radius = 0.0f32;
    for corner in &world_corners {
        radius = radius.max((*corner - center).length());
    }

    let up = if light_dir.y.abs() > 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::UP
    };
    let light_eye = center - light_dir * (radius * 2.0 + 32.0);
    let forward = (center - light_eye).normalize();
    let right = forward.cross(up).normalize();
    let light_up = right.cross(forward);
    let light_z = -forward;
    let tx = -right.dot(light_eye);
    let ty = -light_up.dot(light_eye);
    let tz = -light_z.dot(light_eye);

    let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in &world_corners {
        let light_x = right.dot(*corner) + tx;
        let light_y = light_up.dot(*corner) + ty;
        let light_z_value = light_z.dot(*corner) + tz;
        min.x = min.x.min(light_x);
        min.y = min.y.min(light_y);
        min.z = min.z.min(light_z_value);
        max.x = max.x.max(light_x);
        max.y = max.y.max(light_y);
        max.z = max.z.max(light_z_value);
    }

    let xy_pad = radius * 0.25 + 1.0;
    let z_pad = radius * 2.0 + 32.0;
    let near = (-max.z - z_pad).max(0.1);
    let far = (-min.z + z_pad).max(near + 0.1);
    let left = min.x - xy_pad;
    let right_bound = max.x + xy_pad;
    let bottom = min.y - xy_pad;
    let top = max.y + xy_pad;

    let scale_x = 2.0 / (right_bound - left);
    let scale_y = 2.0 / (top - bottom);
    let scale_z = 1.0 / (near - far);
    let offset_x = (left + right_bound) / (left - right_bound);
    let offset_y = (top + bottom) / (bottom - top);
    let offset_z = near / (near - far);

    [
        [
            scale_x * right.x,
            scale_y * light_up.x,
            scale_z * light_z.x,
            0.0,
        ],
        [
            scale_x * right.y,
            scale_y * light_up.y,
            scale_z * light_z.y,
            0.0,
        ],
        [
            scale_x * right.z,
            scale_y * light_up.z,
            scale_z * light_z.z,
            0.0,
        ],
        [
            scale_x * tx + offset_x,
            scale_y * ty + offset_y,
            scale_z * tz + offset_z,
            1.0,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_mat4_near(actual: &Mat4, expected: &Mat4, eps: f32) {
        for col in 0..4 {
            for row in 0..4 {
                assert!(
                    (actual[col][row] - expected[col][row]).abs() <= eps,
                    "matrix mismatch at [{}, {}]: actual={} expected={}",
                    col,
                    row,
                    actual[col][row],
                    expected[col][row],
                );
            }
        }
    }

    #[test]
    fn decompose_matrix_roundtrips_model_matrix() {
        let original = model_matrix(
            Vec3::new(3.0, -2.0, 7.5),
            Quat::new(0.23426065, -0.3904344, 0.15617377, 0.8769511).normalize(),
            Vec3::new(2.0, 3.5, -4.0),
        );
        let (pos, rot, scale) = decompose_matrix(&original).expect("matrix should decompose");
        let rebuilt = model_matrix(pos, rot, scale);
        assert_mat4_near(&rebuilt, &original, 1e-4);
    }

    #[test]
    fn decompose_matrix_rejects_shear() {
        let sheared = [
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!(
            decompose_matrix(&sheared).is_none(),
            "sheared matrix must fail"
        );
    }

    #[test]
    fn compute_shadow_vp_contains_camera_frustum() {
        let eye = Vec3::new(4.0, 6.0, 10.0);
        let target = Vec3::new(0.0, 1.0, 0.0);
        let up = Vec3::UP;
        let view = look_at_rh(eye, target, up);
        let proj = perspective_rh_zo(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 50.0);
        let view_proj = mat4_mul(&proj, &view);
        let inv_view_proj = mat4_inverse(&view_proj).expect("view-projection should invert");
        let shadow_vp = compute_shadow_vp(&inv_view_proj, Vec3::new(0.3, -1.0, 0.2).normalize());
        let ndc_corners = [
            [-1.0, -1.0, 0.0, 1.0],
            [-1.0, 1.0, 0.0, 1.0],
            [1.0, -1.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [-1.0, -1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0, 1.0],
            [1.0, -1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ];

        for ndc in ndc_corners {
            let world = mat4_mul_vec4(&inv_view_proj, ndc);
            let inv_w = 1.0 / world[3];
            let shadow_clip = mat4_mul_vec4(
                &shadow_vp,
                [world[0] * inv_w, world[1] * inv_w, world[2] * inv_w, 1.0],
            );
            let shadow_ndc = [
                shadow_clip[0] / shadow_clip[3],
                shadow_clip[1] / shadow_clip[3],
                shadow_clip[2] / shadow_clip[3],
            ];
            assert!(
                shadow_ndc[0] >= -1.0 && shadow_ndc[0] <= 1.0,
                "shadow x out of range: {}",
                shadow_ndc[0]
            );
            assert!(
                shadow_ndc[1] >= -1.0 && shadow_ndc[1] <= 1.0,
                "shadow y out of range: {}",
                shadow_ndc[1]
            );
            assert!(
                shadow_ndc[2] >= 0.0 && shadow_ndc[2] <= 1.0,
                "shadow z out of range: {}",
                shadow_ndc[2]
            );
        }
    }
}
