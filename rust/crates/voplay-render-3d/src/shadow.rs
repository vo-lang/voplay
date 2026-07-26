use crate::{
    CameraFrame3d, CameraProjection3d, LightKind, LightPacket3d, Render3dError, ShadowCascade,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalShadowCascade3d {
    pub descriptor: ShadowCascade,
    pub light_view_projection: [[f32; 4]; 4],
}

pub fn build_directional_shadow_cascades(
    camera: CameraFrame3d,
    lights: &[LightPacket3d],
    viewport_width: u32,
    viewport_height: u32,
    shadow_resolution: u32,
    max_cascades: usize,
) -> Result<Vec<DirectionalShadowCascade3d>, Render3dError> {
    if viewport_width == 0
        || viewport_height == 0
        || shadow_resolution == 0
        || max_cascades == 0
        || max_cascades > 4
    {
        return Err(Render3dError::InvalidConfig);
    }
    let Some(light) = lights.iter().find(|light| {
        light.descriptor.casts_shadow && matches!(light.descriptor.kind, LightKind::Directional)
    }) else {
        return Ok(Vec::new());
    };
    let (near_milli, far_milli) = match camera.descriptor.projection {
        CameraProjection3d::Perspective {
            near_milli,
            far_milli,
            ..
        }
        | CameraProjection3d::Orthographic {
            near_milli,
            far_milli,
            ..
        } => (u64::from(near_milli), u64::from(far_milli)),
    };
    if far_milli <= near_milli {
        return Err(Render3dError::InvalidDescriptor);
    }
    let light_direction = normalize_milli(light.direction_milli)?;
    let camera_forward = normalize_milli(camera.forward_milli)?;
    let aspect = viewport_width as f32 / viewport_height as f32;
    let cascade_count = max_cascades.min(4);
    let splits = practical_splits(near_milli, far_milli, cascade_count);
    let mut result = Vec::with_capacity(cascade_count);
    let mut cascade_near = near_milli;
    for (index, cascade_far) in splits.into_iter().enumerate() {
        let center_distance = (cascade_near as f32 + cascade_far as f32) * 0.0005;
        let center = [
            camera.position_milli[0] as f32 / 1_000.0 + camera_forward[0] * center_distance,
            camera.position_milli[1] as f32 / 1_000.0 + camera_forward[1] * center_distance,
            camera.position_milli[2] as f32 / 1_000.0 + camera_forward[2] * center_distance,
        ];
        let radius = cascade_radius(
            camera.descriptor.projection,
            aspect,
            cascade_near,
            cascade_far,
        )
        .max(0.001);
        let texel_world = (radius * 2.0 / shadow_resolution as f32).max(0.001);
        let snapped_center = center.map(|value| (value / texel_world).floor() * texel_world);
        let eye = [
            snapped_center[0] - light_direction[0] * radius * 2.0,
            snapped_center[1] - light_direction[1] * radius * 2.0,
            snapped_center[2] - light_direction[2] * radius * 2.0,
        ];
        let view = look_at(eye, snapped_center, light_direction)?;
        let projection = orthographic(radius, radius * 4.0)?;
        let light_view_projection = multiply(projection, view);
        if !light_view_projection
            .iter()
            .flatten()
            .all(|value| value.is_finite())
        {
            return Err(Render3dError::ArithmeticOverflow);
        }
        result.push(DirectionalShadowCascade3d {
            descriptor: ShadowCascade {
                index: index as u32,
                near_milli: cascade_near,
                far_milli: cascade_far,
                texel_world_milli: (texel_world * 1_000.0).ceil().max(1.0) as u64,
            },
            light_view_projection,
        });
        cascade_near = cascade_far;
    }
    Ok(result)
}

fn practical_splits(near: u64, far: u64, count: usize) -> Vec<u64> {
    let near_f = near as f64;
    let far_f = far as f64;
    (1..=count)
        .map(|index| {
            if index == count {
                return far;
            }
            let ratio = index as f64 / count as f64;
            let logarithmic = near_f * (far_f / near_f).powf(ratio);
            let uniform = near_f + (far_f - near_f) * ratio;
            ((logarithmic + uniform) * 0.5)
                .round()
                .clamp(near_f + 1.0, far_f - 1.0) as u64
        })
        .collect()
}

fn cascade_radius(
    projection: CameraProjection3d,
    aspect: f32,
    near_milli: u64,
    far_milli: u64,
) -> f32 {
    let half_depth = (far_milli - near_milli) as f32 / 2_000.0;
    match projection {
        CameraProjection3d::Perspective {
            vertical_fov_millidegrees,
            ..
        } => {
            let far = far_milli as f32 / 1_000.0;
            let half_height =
                far * ((vertical_fov_millidegrees as f32 / 1_000.0).to_radians() * 0.5).tan();
            let half_width = half_height * aspect;
            (half_width * half_width + half_height * half_height + half_depth * half_depth).sqrt()
        }
        CameraProjection3d::Orthographic {
            vertical_height_milli,
            ..
        } => {
            let half_height = vertical_height_milli as f32 / 2_000.0;
            let half_width = half_height * aspect;
            (half_width * half_width + half_height * half_height + half_depth * half_depth).sqrt()
        }
    }
}

fn normalize_milli(value: [i64; 3]) -> Result<[f32; 3], Render3dError> {
    let value = value.map(|component| component as f32 / 1_000.0);
    let length = value
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(Render3dError::InvalidDescriptor);
    }
    Ok(value.map(|component| component / length))
}

fn look_at(
    eye: [f32; 3],
    center: [f32; 3],
    light_direction: [f32; 3],
) -> Result<[[f32; 4]; 4], Render3dError> {
    let forward = normalize([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]])?;
    let preferred_up = if light_direction[1].abs() > 0.95 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(forward, preferred_up))?;
    let up = cross(right, forward);
    Ok([
        [right[0], up[0], -forward[0], 0.0],
        [right[1], up[1], -forward[1], 0.0],
        [right[2], up[2], -forward[2], 0.0],
        [-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0],
    ])
}

fn orthographic(radius: f32, depth: f32) -> Result<[[f32; 4]; 4], Render3dError> {
    if !radius.is_finite() || !depth.is_finite() || radius <= 0.0 || depth <= 0.0 {
        return Err(Render3dError::ArithmeticOverflow);
    }
    Ok([
        [1.0 / radius, 0.0, 0.0, 0.0],
        [0.0, 1.0 / radius, 0.0, 0.0],
        [0.0, 0.0, -1.0 / depth, 0.0],
        [0.0, 0.0, 0.5, 1.0],
    ])
}

fn normalize(value: [f32; 3]) -> Result<[f32; 3], Render3dError> {
    let length = value
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(Render3dError::ArithmeticOverflow);
    }
    Ok(value.map(|component| component / length))
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn multiply(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            result[column][row] = (0..4)
                .map(|axis| left[axis][row] * right[column][axis])
                .sum();
        }
    }
    result
}
