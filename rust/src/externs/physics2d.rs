//! Physics 2D externs (voplay/scene2d sub-package).

use vo_ext::prelude::*;
use crate::physics::{BodyDesc, ColliderKind};
use crate::physics_registry::PhysBodyType;

// --- Physics 2D world management ---

#[vo_fn("voplay/scene2d", "physicsInit")]
pub fn physics_init(call: &mut ExternCallContext) -> ExternResult {
    let gx = call.arg_f64(0) as f32;
    let gy = call.arg_f64(1) as f32;
    let world_id = crate::physics::create_world(gx, gy);
    call.ret_u64(0, world_id as u64);
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsDestroy")]
pub fn physics_destroy(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    crate::physics::destroy_world(world_id);
    ExternResult::Ok
}

/// Decode a BodyDesc from bytes.
/// Format: body_type(u8), collider_kind(u8), fixed_rotation(u8), layer(u16), mask(u16),
///         x(f64), y(f64), rotation(f64),
///         collider_args(3x f64), density(f64), friction(f64), restitution(f64),
///         linear_damping(f64)
fn decode_body_desc(body_id: u32, data: &[u8]) -> BodyDesc {
    // 3 flag bytes + 2 u16 fields + 10 f64 fields = 87 bytes minimum
    assert!(data.len() >= 87, "voplay: physics2d body desc too short: {} bytes (expected 87)", data.len());
    let mut pos = 0;
    let body_type = PhysBodyType::from_u8(data[pos]);
    pos += 1;
    let collider_kind = match data[pos] {
        2 => ColliderKind::Circle,
        3 => ColliderKind::Capsule,
        _ => ColliderKind::Box,
    };
    pos += 1;
    let fixed_rotation = data[pos] != 0;
    pos += 1;
    let layer = u16::from_le_bytes([data[pos], data[pos + 1]]);
    pos += 2;
    let mask = u16::from_le_bytes([data[pos], data[pos + 1]]);
    pos += 2;

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
    let rotation = read_f64(&mut pos) as f32;
    let ca0 = read_f64(&mut pos) as f32;
    let ca1 = read_f64(&mut pos) as f32;
    let ca2 = read_f64(&mut pos) as f32;
    let density = read_f64(&mut pos) as f32;
    let friction = read_f64(&mut pos) as f32;
    let restitution = read_f64(&mut pos) as f32;
    let linear_damping = read_f64(&mut pos) as f32;

    BodyDesc {
        body_id,
        body_type,
        x, y, rotation,
        collider_kind,
        collider_args: [ca0, ca1, ca2],
        layer,
        mask,
        density,
        friction,
        restitution,
        linear_damping,
        fixed_rotation,
    }
}

#[vo_fn("voplay/scene2d", "physicsSpawnBody")]
pub fn physics_spawn_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    let data = call.arg_bytes(2);
    let desc = decode_body_desc(body_id, data);
    crate::physics::with_world(world_id, |world| world.spawn_body(&desc));
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsDestroyBody")]
pub fn physics_destroy_body(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let body_id = call.arg_u64(1) as u32;
    crate::physics::with_world(world_id, |world| world.destroy_body(body_id));
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsStep")]
pub fn physics_step(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let dt = call.arg_f64(1) as f32;
    let cmds = call.arg_bytes(2);
    let cmds_owned = cmds.to_vec();

    let state = crate::physics::with_world(world_id, |world| {
        world.apply_commands(&cmds_owned);
        world.step(dt);
        world.serialize_state()
    });

    let slice_ref = call.alloc_bytes(&state);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsSetGravity")]
pub fn physics_set_gravity(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let gx = call.arg_f64(1) as f32;
    let gy = call.arg_f64(2) as f32;
    crate::physics::with_world(world_id, |world| world.set_gravity(gx, gy));
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsContacts")]
pub fn physics_contacts(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let contacts = crate::physics::with_world(world_id, |world| world.get_contacts());
    // Serialize: count(u32), then per pair: body_id_a(u32), body_id_b(u32)
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

// --- Physics 2D query externs ---

#[vo_fn("voplay/scene2d", "physicsRayCast")]
pub fn physics_ray_cast(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let ox = call.arg_f64(1) as f32;
    let oy = call.arg_f64(2) as f32;
    let dx = call.arg_f64(3) as f32;
    let dy = call.arg_f64(4) as f32;
    let max_dist = call.arg_f64(5) as f32;

    let result = crate::physics::with_world(world_id, |world| world.ray_cast(ox, oy, dx, dy, max_dist));
    match result {
        Some((body_id, hx, hy, nx, ny, toi)) => {
            // Serialize: found(u8=1), body_id(u32), hx(f64), hy(f64), nx(f64), ny(f64), dist(f64)
            let mut buf = Vec::with_capacity(45);
            buf.push(1u8);
            buf.extend_from_slice(&body_id.to_le_bytes());
            buf.extend_from_slice(&(hx as f64).to_le_bytes());
            buf.extend_from_slice(&(hy as f64).to_le_bytes());
            buf.extend_from_slice(&(nx as f64).to_le_bytes());
            buf.extend_from_slice(&(ny as f64).to_le_bytes());
            buf.extend_from_slice(&(toi as f64).to_le_bytes());
            let slice_ref = call.alloc_bytes(&buf);
            call.ret_ref(0, slice_ref);
        }
        None => {
            let buf = [0u8]; // not found
            let slice_ref = call.alloc_bytes(&buf);
            call.ret_ref(0, slice_ref);
        }
    }
    ExternResult::Ok
}

#[vo_fn("voplay/scene2d", "physicsQueryRect")]
pub fn physics_query_rect(call: &mut ExternCallContext) -> ExternResult {
    let world_id = call.arg_u64(0) as u32;
    let min_x = call.arg_f64(1) as f32;
    let min_y = call.arg_f64(2) as f32;
    let max_x = call.arg_f64(3) as f32;
    let max_y = call.arg_f64(4) as f32;

    let ids = crate::physics::with_world(world_id, |world| world.query_rect(min_x, min_y, max_x, max_y));
    // Serialize: count(u32), then body_id(u32) per hit
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let slice_ref = call.alloc_bytes(&buf);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}
