use super::*;

// ── scene2d physics externs ───────────────────────────────────────────────────

/// scene2d_physicsInit(gx, gy) → uint32
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsInit")]
pub fn scene2d_physics_init(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    pos.finish();
    let world_id = crate::physics::create_world(gx, gy);
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// scene2d_physicsDestroy(worldId)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsDestroy")]
pub fn scene2d_physics_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    pos.finish();
    crate::physics::destroy_world(world_id);
    Vec::new()
}

/// scene2d_physicsSpawnBody(worldId, bodyId, data []byte)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsSpawnBody")]
pub fn scene2d_physics_spawn_body(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    pos.finish();
    let desc = crate::externs::physics2d::decode_body_desc(body_id, data);
    crate::physics::with_world(world_id, |world| world.spawn_body(&desc));
    Vec::new()
}

/// scene2d_physicsDestroyBody(worldId, bodyId)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsDestroyBody")]
pub fn scene2d_physics_destroy_body(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    pos.finish();
    crate::physics::with_world(world_id, |world| world.destroy_body(body_id));
    Vec::new()
}

/// scene2d_physicsStep(worldId, dt, cmds []byte) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsStep")]
pub fn scene2d_physics_step(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let cmds = in_bytes(input, &mut pos).to_vec();
    pos.finish();
    let state = crate::physics::with_world(world_id, |world| {
        world
            .apply_commands(&cmds)
            .unwrap_or_else(|error| panic!("{error}"));
        world.step(dt);
        world.serialize_state()
    });
    let mut out = Vec::new();
    out_bytes(&mut out, &state);
    out
}

/// scene2d_physicsSetGravity(worldId, gx, gy)
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsSetGravity")]
pub fn scene2d_physics_set_gravity(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    pos.finish();
    crate::physics::with_world(world_id, |world| world.set_gravity(gx, gy));
    Vec::new()
}

/// scene2d_physicsContacts(worldId) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsContacts")]
pub fn scene2d_physics_contacts(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    pos.finish();
    let contacts = crate::physics::with_world(world_id, |world| world.get_contacts());
    let mut buf = Vec::with_capacity(4 + contacts.len() * 8);
    buf.extend_from_slice(&(contacts.len() as u32).to_le_bytes());
    for (a, b) in &contacts {
        buf.extend_from_slice(&a.to_le_bytes());
        buf.extend_from_slice(&b.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene2d_physicsRayCast(worldId, ox, oy, dx, dy, maxDist) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsRayCast")]
pub fn scene2d_physics_ray_cast(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let ox = in_f64(input, &mut pos) as f32;
    let oy = in_f64(input, &mut pos) as f32;
    let dx = in_f64(input, &mut pos) as f32;
    let dy = in_f64(input, &mut pos) as f32;
    let max_dist = in_f64(input, &mut pos) as f32;
    pos.finish();
    let result =
        crate::physics::with_world(world_id, |world| world.ray_cast(ox, oy, dx, dy, max_dist));
    let buf = match result {
        Some((body_id, hx, hy, nx, ny, toi)) => {
            let mut b = Vec::with_capacity(45);
            b.push(1u8);
            b.extend_from_slice(&body_id.to_le_bytes());
            b.extend_from_slice(&(hx as f64).to_le_bytes());
            b.extend_from_slice(&(hy as f64).to_le_bytes());
            b.extend_from_slice(&(nx as f64).to_le_bytes());
            b.extend_from_slice(&(ny as f64).to_le_bytes());
            b.extend_from_slice(&(toi as f64).to_le_bytes());
            b
        }
        None => vec![0u8],
    };
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene2d_physicsQueryRect(worldId, minX, minY, maxX, maxY) → []byte
#[vo_ext::vo_wasm_bindgen_export("voplay/scene2d", "physicsQueryRect")]
pub fn scene2d_physics_query_rect(input: &[u8]) -> Vec<u8> {
    let mut pos = DecodePosition::new(input);
    let world_id = in_value(input, &mut pos) as u32;
    let min_x = in_f64(input, &mut pos) as f32;
    let min_y = in_f64(input, &mut pos) as f32;
    let max_x = in_f64(input, &mut pos) as f32;
    let max_y = in_f64(input, &mut pos) as f32;
    pos.finish();
    let ids = crate::physics::with_world(world_id, |world| {
        world.query_rect(min_x, min_y, max_x, max_y)
    });
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}
