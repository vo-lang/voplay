//! Physics 3D externs (voplay/scene3d sub-package).

use vo_ext::prelude::*;
use crate::physics3d::{BodyDesc3D, ColliderKind3D};
use crate::physics_registry::PhysBodyType;

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
    crate::physics3d::destroy_world(world_id);
    ExternResult::Ok
}

/// Decode a 3D BodyDesc from bytes.
/// Format: body_type(u8), collider_kind(u8), fixed_rotation(u8),
///         x(f64), y(f64), z(f64), qx(f64), qy(f64), qz(f64), qw(f64),
///         collider_args(3x f64), density(f64), friction(f64), restitution(f64),
///         linear_damping(f64)
fn decode_body3d_desc(body_id: u32, data: &[u8]) -> BodyDesc3D {
    // 3 flag bytes + 14 f64 fields = 115 bytes minimum
    assert!(data.len() >= 115, "voplay: physics3d body desc too short: {} bytes (expected 115)", data.len());
    let mut pos = 0;
    let body_type = PhysBodyType::from_u8(data[pos]);
    pos += 1;
    let collider_kind = match data[pos] {
        5 => ColliderKind3D::Sphere,
        3 => ColliderKind3D::Capsule,
        _ => ColliderKind3D::Box3D, // kind=4 (Box3D) is default
    };
    pos += 1;
    let fixed_rotation = data[pos] != 0;
    pos += 1;

    let read_f64 = |p: &mut usize| -> f64 {
        let v = f64::from_le_bytes([
            data[*p], data[*p+1], data[*p+2], data[*p+3],
            data[*p+4], data[*p+5], data[*p+6], data[*p+7],
        ]);
        *p += 8;
        v
    };

    let x = read_f64(&mut pos) as f32;
    let y = read_f64(&mut pos) as f32;
    let z = read_f64(&mut pos) as f32;
    let qx = read_f64(&mut pos) as f32;
    let qy = read_f64(&mut pos) as f32;
    let qz = read_f64(&mut pos) as f32;
    let qw = read_f64(&mut pos) as f32;
    let ca0 = read_f64(&mut pos) as f32;
    let ca1 = read_f64(&mut pos) as f32;
    let ca2 = read_f64(&mut pos) as f32;
    let density = read_f64(&mut pos) as f32;
    let friction = read_f64(&mut pos) as f32;
    let restitution = read_f64(&mut pos) as f32;
    let linear_damping = read_f64(&mut pos) as f32;

    BodyDesc3D {
        body_id,
        body_type,
        x, y, z,
        qx, qy, qz, qw,
        collider_kind,
        collider_args: [ca0, ca1, ca2],
        density,
        friction,
        restitution,
        linear_damping,
        fixed_rotation,
    }
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

#[vo_fn("voplay/scene3d", "physicsDestroyBody")]
pub fn physics3d_destroy_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
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

    let slice_ref = call.alloc_bytes(&state);
    call.ret_ref(0, slice_ref);
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
    let slice_ref = call.alloc_bytes(&buf);
    call.ret_ref(0, slice_ref);
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

    let result = crate::physics3d::with_world(world_id, |world| world.ray_cast(ox, oy, oz, dx, dy, dz, max_dist));
    match result {
        Some((body_id, hx, hy, hz, nx, ny, nz, toi)) => {
            // Serialize: found(u8=1), body_id(u32), hx(f64), hy(f64), hz(f64), nx(f64), ny(f64), nz(f64), dist(f64)
            let mut buf = Vec::with_capacity(61);
            buf.push(1u8);
            buf.extend_from_slice(&body_id.to_le_bytes());
            buf.extend_from_slice(&(hx as f64).to_le_bytes());
            buf.extend_from_slice(&(hy as f64).to_le_bytes());
            buf.extend_from_slice(&(hz as f64).to_le_bytes());
            buf.extend_from_slice(&(nx as f64).to_le_bytes());
            buf.extend_from_slice(&(ny as f64).to_le_bytes());
            buf.extend_from_slice(&(nz as f64).to_le_bytes());
            buf.extend_from_slice(&(toi as f64).to_le_bytes());
            let slice_ref = call.alloc_bytes(&buf);
            call.ret_ref(0, slice_ref);
        }
        None => {
            let buf = [0u8];
            let slice_ref = call.alloc_bytes(&buf);
            call.ret_ref(0, slice_ref);
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

    let ids = crate::physics3d::with_world(world_id, |world| world.query_aabb(min_x, min_y, min_z, max_x, max_y, max_z));
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let slice_ref = call.alloc_bytes(&buf);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}
