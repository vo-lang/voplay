//! Physics 3D externs (voplay/scene3d sub-package).

use vo_ext::prelude::*;
use crate::math3d::{Vec3, Quat};
use crate::physics3d::{BodyDesc3D, ColliderKind3D, HeightfieldDesc3D, TrimeshDesc3D};
use crate::physics_registry::PhysBodyType;
use super::util::{read_f64_le, read_u16_le, read_u32_le, ret_bytes, with_renderer_or_panic};

// --- Physics 3D world management ---

#[vo_fn("voplay/scene3d", "physicsInit")]
pub fn physics3d_init(call: &mut ExternCallContext) -> ExternResult {
    let gx = call.arg_f64(0) as f32;
    let gy = call.arg_f64(1) as f32;
    let gz = call.arg_f64(2) as f32;
    let world_id = crate::physics3d::create_world(gx, gy, gz);
    call.ret_u64(0, world_id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsDestroy")]
pub fn physics3d_destroy(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    crate::terrain::remove_world(world_id);
    crate::physics3d::destroy_world(world_id);
    ExternResult::Ok
}

/// Decode a 3D BodyDesc from bytes.
/// Format: body_type(u8), collider_kind(u8), fixed_rotation(u8), layer(u16), mask(u16),
///         x(f64), y(f64), z(f64), qx(f64), qy(f64), qz(f64), qw(f64),
///         collider_args(3x f64), collider_offset(3x f64), density(f64), friction(f64), restitution(f64),
///         linear_damping(f64)
pub(crate) fn decode_body3d_desc(body_id: u32, data: &[u8]) -> BodyDesc3D {
    // 3 flag bytes + 2 u16 fields + 17 f64 fields = 143 bytes minimum
    assert!(data.len() >= 143, "voplay: physics3d body desc too short: {} bytes (expected 143)", data.len());
    let mut pos = 0;
    let body_type = PhysBodyType::from_u8(data[pos]);
    pos += 1;
    let collider_kind = match data[pos] {
        4 => ColliderKind3D::Box3D,
        5 => ColliderKind3D::Sphere,
        3 => ColliderKind3D::Capsule,
        other => panic!("voplay: unknown 3D collider kind: {}", other),
    };
    pos += 1;
    let fixed_rotation = data[pos] != 0;
    pos += 1;
    let layer = read_u16_le(data, &mut pos);
    let mask = read_u16_le(data, &mut pos);

    let x = read_f64_le(data, &mut pos) as f32;
    let y = read_f64_le(data, &mut pos) as f32;
    let z = read_f64_le(data, &mut pos) as f32;
    let qx = read_f64_le(data, &mut pos) as f32;
    let qy = read_f64_le(data, &mut pos) as f32;
    let qz = read_f64_le(data, &mut pos) as f32;
    let qw = read_f64_le(data, &mut pos) as f32;
    let ca0 = read_f64_le(data, &mut pos) as f32;
    let ca1 = read_f64_le(data, &mut pos) as f32;
    let ca2 = read_f64_le(data, &mut pos) as f32;
    let off_x = read_f64_le(data, &mut pos) as f32;
    let off_y = read_f64_le(data, &mut pos) as f32;
    let off_z = read_f64_le(data, &mut pos) as f32;
    let density = read_f64_le(data, &mut pos) as f32;
    let friction = read_f64_le(data, &mut pos) as f32;
    let restitution = read_f64_le(data, &mut pos) as f32;
    let linear_damping = read_f64_le(data, &mut pos) as f32;

    BodyDesc3D {
        body_id,
        body_type,
        pos: Vec3::new(x, y, z),
        rot: Quat::new(qx, qy, qz, qw),
        collider_kind,
        collider_args: [ca0, ca1, ca2],
        collider_offset: Vec3::new(off_x, off_y, off_z),
        layer,
        mask,
        density,
        friction,
        restitution,
        linear_damping,
        fixed_rotation,
    }
}

pub(crate) fn decode_trimesh_desc(body_id: u32, data: &[u8]) -> TrimeshDesc3D {
    assert!(
        data.len() >= 84,
        "voplay: physics3d trimesh desc too short: {} bytes (expected 84)",
        data.len()
    );
    let mut pos = 0;

    let x = read_f64_le(data, &mut pos) as f32;
    let y = read_f64_le(data, &mut pos) as f32;
    let z = read_f64_le(data, &mut pos) as f32;
    let qx = read_f64_le(data, &mut pos) as f32;
    let qy = read_f64_le(data, &mut pos) as f32;
    let qz = read_f64_le(data, &mut pos) as f32;
    let qw = read_f64_le(data, &mut pos) as f32;
    let scale_x = read_f64_le(data, &mut pos) as f32;
    let scale_y = read_f64_le(data, &mut pos) as f32;
    let scale_z = read_f64_le(data, &mut pos) as f32;
    let layer = read_u16_le(data, &mut pos);
    let mask = read_u16_le(data, &mut pos);
    let friction = read_f64_le(data, &mut pos) as f32;
    let restitution = read_f64_le(data, &mut pos) as f32;

    TrimeshDesc3D {
        body_id,
        pos: Vec3::new(x, y, z),
        rot: Quat::new(qx, qy, qz, qw),
        scale: Vec3::new(scale_x, scale_y, scale_z),
        layer,
        mask,
        friction,
        restitution,
    }
}

pub(crate) fn decode_model_mesh_data_bytes(data: &[u8]) -> (Vec<[f32; 3]>, Vec<u32>) {
    assert!(data.len() >= 8, "voplay: model mesh data too short: {}", data.len());
    let mut pos = 0usize;
    let position_count = read_u32_le(data, &mut pos) as usize;
    let index_count = read_u32_le(data, &mut pos) as usize;
    let expected_len = 8 + position_count * 12 + index_count * 4;
    assert!(
        data.len() == expected_len,
        "voplay: model mesh data size mismatch: got {}, expected {}",
        data.len(),
        expected_len
    );
    let mut positions = Vec::with_capacity(position_count);
    for _ in 0..position_count {
        positions.push([
            f32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]),
            f32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]),
            f32::from_le_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]]),
        ]);
        pos += 12;
    }
    let mut indices = Vec::with_capacity(index_count);
    for _ in 0..index_count {
        indices.push(read_u32_le(data, &mut pos));
    }
    (positions, indices)
}

pub(crate) fn spawn_trimesh_body_from_mesh_data(
    world_id: u32,
    body_id: u32,
    data: &[u8],
    mesh_data: &[u8],
) {
    let desc = decode_trimesh_desc(body_id, data);
    let (positions, indices) = decode_model_mesh_data_bytes(mesh_data);
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_trimesh_body(&desc, &positions, &indices)
    });
}

fn decode_heightfield_data(data: &[u8]) -> Vec<f32> {
    assert!(
        data.len() % 4 == 0,
        "voplay: heightfield bytes length must be a multiple of 4, got {}",
        data.len()
    );
    data.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[vo_fn("voplay/scene3d", "physicsSpawnBody")]
pub fn physics3d_spawn_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let data = call.arg_bytes(2);
    let desc = decode_body3d_desc(body_id, data);
    crate::physics3d::with_world(world_id, |world| world.spawn_body(&desc));
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsSpawnTrimeshBody")]
pub fn physics3d_spawn_trimesh_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let model_id = call.arg_u64(2) as u32;
    let data = call.arg_bytes(3);
    let desc = decode_trimesh_desc(body_id, data);
    let mesh_data = with_renderer_or_panic("physicsSpawnTrimeshBody", |renderer| {
        renderer.get_model_mesh_data(model_id)
    });
    let (positions, indices) = match mesh_data {
        Some(mesh_data) => mesh_data,
        None => panic!("physicsSpawnTrimeshBody: model not found: {}", model_id),
    };
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_trimesh_body(&desc, &positions, &indices)
    });
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsSpawnTrimeshBodyData")]
pub fn physics3d_spawn_trimesh_body_data(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let data = call.arg_bytes(2);
    let mesh_data = call.arg_bytes(3);
    spawn_trimesh_body_from_mesh_data(world_id, body_id, data, mesh_data);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsSpawnHeightfield")]
pub fn physics3d_spawn_heightfield(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let height_data = decode_heightfield_data(call.arg_bytes(2));
    let rows = call.arg_u64(3) as u32;
    let cols = call.arg_u64(4) as u32;
    let scale_x = call.arg_f64(5) as f32;
    let scale_y = call.arg_f64(6) as f32;
    let scale_z = call.arg_f64(7) as f32;
    let pos = Vec3::new(
        call.arg_f64(8) as f32,
        call.arg_f64(9) as f32,
        call.arg_f64(10) as f32,
    );
    let layer = call.arg_u64(11) as u16;
    let mask = call.arg_u64(12) as u16;
    let friction = call.arg_f64(13) as f32;
    let restitution = call.arg_f64(14) as f32;
    let desc = HeightfieldDesc3D {
        body_id,
        pos,
        layer,
        mask,
        friction,
        restitution,
        rows,
        cols,
        scale_x,
        scale_y,
        scale_z,
    };
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_heightfield_body(&desc, &height_data)
    });
    crate::terrain::store_terrain(
        world_id,
        body_id,
        pos,
        crate::terrain::TerrainData {
            model_id: 0,
            heights: height_data,
            rows,
            cols,
            scale_x,
            scale_y,
            scale_z,
            origin: pos,
        },
    );
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsDestroyBody")]
pub fn physics3d_destroy_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    crate::terrain::remove_terrain(world_id, body_id);
    crate::physics3d::with_world(world_id, |world| world.destroy_body(body_id));
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsStep")]
pub fn physics3d_step(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let dt = call.arg_f64(1) as f32;
    let cmds = call.arg_bytes(2);
    let cmds_owned = cmds.to_vec();

    let state = crate::physics3d::with_world(world_id, |world| {
        world.apply_commands(&cmds_owned);
        world.step(dt);
        world.serialize_state()
    });

    ret_bytes(call, 0, &state);
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsSetGravity")]
pub fn physics3d_set_gravity(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let gx = call.arg_f64(1) as f32;
    let gy = call.arg_f64(2) as f32;
    let gz = call.arg_f64(3) as f32;
    crate::physics3d::with_world(world_id, |world| world.set_gravity(gx, gy, gz));
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsContacts")]
pub fn physics3d_contacts(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let contacts = crate::physics3d::with_world(world_id, |world| world.get_contacts());
    let mut buf = Vec::with_capacity(4 + contacts.len() * 8);
    buf.extend_from_slice(&(contacts.len() as u32).to_le_bytes());
    for (a, b) in &contacts {
        buf.extend_from_slice(&a.to_le_bytes());
        buf.extend_from_slice(&b.to_le_bytes());
    }
    ret_bytes(call, 0, &buf);
    ExternResult::Ok
}

// --- Physics 3D query externs ---

#[vo_fn("voplay/scene3d", "physicsRayCast")]
pub fn physics3d_ray_cast(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let ox = call.arg_f64(1) as f32;
    let oy = call.arg_f64(2) as f32;
    let oz = call.arg_f64(3) as f32;
    let dx = call.arg_f64(4) as f32;
    let dy = call.arg_f64(5) as f32;
    let dz = call.arg_f64(6) as f32;
    let max_dist = call.arg_f64(7) as f32;

    let origin = Vec3::new(ox, oy, oz);
    let dir = Vec3::new(dx, dy, dz);
    let result = crate::physics3d::with_world(world_id, |world| world.ray_cast(origin, dir, max_dist));
    match result {
        Some(hit) => {
            // Serialize: found(u8=1), body_id(u32), hx(f64), hy(f64), hz(f64), nx(f64), ny(f64), nz(f64), dist(f64)
            let mut buf = Vec::with_capacity(61);
            buf.push(1u8);
            buf.extend_from_slice(&hit.body_id.to_le_bytes());
            buf.extend_from_slice(&(hit.point.x as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.point.y as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.point.z as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.normal.x as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.normal.y as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.normal.z as f64).to_le_bytes());
            buf.extend_from_slice(&(hit.toi as f64).to_le_bytes());
            ret_bytes(call, 0, &buf);
        }
        None => {
            let buf = [0u8];
            ret_bytes(call, 0, &buf);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay/scene3d", "physicsQueryAABB")]
pub fn physics3d_query_aabb(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let min_x = call.arg_f64(1) as f32;
    let min_y = call.arg_f64(2) as f32;
    let min_z = call.arg_f64(3) as f32;
    let max_x = call.arg_f64(4) as f32;
    let max_y = call.arg_f64(5) as f32;
    let max_z = call.arg_f64(6) as f32;

    let min = Vec3::new(min_x, min_y, min_z);
    let max = Vec3::new(max_x, max_y, max_z);
    let ids = crate::physics3d::with_world(world_id, |world| world.query_aabb(min, max));
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    ret_bytes(call, 0, &buf);
    ExternResult::Ok
}
