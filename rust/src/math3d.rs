//! GPU-side math utilities for 3D and 2D rendering.
//!
//! All matrices are in **column-major** layout (wgpu convention).
//! Provides: Vec3 ops, Mat4 construction (model, view, projection),
//! quaternion-to-matrix, and orthographic/perspective projections.

// ─── Vec3 operations ────────────────────────────────────────────────────────

/// Component-wise subtraction.
pub fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Component-wise addition.
pub fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Uniform scale.
pub fn scale3(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

/// Dot product.
pub fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
pub fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Length of a 3D vector.
pub fn length3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Normalize a 3D vector. Returns zero vector if length is zero.
pub fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = length3(v);
    if l == 0.0 {
        return [0.0; 3];
    }
    [v[0] / l, v[1] / l, v[2] / l]
}

// ─── Mat4 operations ────────────────────────────────────────────────────────

/// The 4×4 identity matrix.
pub const MAT4_IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Multiply two 4×4 matrices (column-major).
pub fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[i][j] =
                a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + a[i][3] * b[3][j];
        }
    }
    out
}

/// Multiply a 4×4 matrix (column-major) by a vec4.
pub fn mat4_mul_vec4(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0] * v[3],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1] * v[3],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2] * v[3],
        m[0][3] * v[0] + m[1][3] * v[1] + m[2][3] * v[2] + m[3][3] * v[3],
    ]
}

/// Transpose of the upper-left 3×3, stored in a 4×4 (for normal matrix).
pub fn transpose_upper3x3(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    [
        [m[0][0], m[1][0], m[2][0], 0.0],
        [m[0][1], m[1][1], m[2][1], 0.0],
        [m[0][2], m[1][2], m[2][2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

// ─── Model / View / Projection ──────────────────────────────────────────────

/// Build a 4×4 model matrix from position, quaternion rotation, and scale.
pub fn model_matrix(
    px: f32,
    py: f32,
    pz: f32,
    qx: f32,
    qy: f32,
    qz: f32,
    qw: f32,
    sx: f32,
    sy: f32,
    sz: f32,
) -> [[f32; 4]; 4] {
    let x2 = qx + qx;
    let y2 = qy + qy;
    let z2 = qz + qz;
    let xx = qx * x2;
    let xy = qx * y2;
    let xz = qx * z2;
    let yy = qy * y2;
    let yz = qy * z2;
    let zz = qz * z2;
    let wx = qw * x2;
    let wy = qw * y2;
    let wz = qw * z2;

    [
        [(1.0 - (yy + zz)) * sx, (xy + wz) * sx, (xz - wy) * sx, 0.0],
        [(xy - wz) * sy, (1.0 - (xx + zz)) * sy, (yz + wx) * sy, 0.0],
        [(xz + wy) * sz, (yz - wx) * sz, (1.0 - (xx + yy)) * sz, 0.0],
        [px, py, pz, 1.0],
    ]
}

/// Build a right-handed look-at view matrix (column-major).
pub fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize3(sub3(target, eye));
    let s = normalize3(cross3(f, up));
    let u = cross3(s, f);

    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot3(s, eye), -dot3(u, eye), dot3(f, eye), 1.0],
    ]
}

/// Right-handed perspective projection with zero-to-one depth range (wgpu convention).
pub fn perspective_rh_zo(fov_y_rad: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
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

/// Column-major orthographic projection matrix.
pub fn orthographic(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> [[f32; 4]; 4] {
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
