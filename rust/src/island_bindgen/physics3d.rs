use super::*;

// ── scene3d physics externs ───────────────────────────────────────────────────

/// scene3d_physicsInit(gx, gy, gz) → uint32
#[wasm_bindgen(js_name = "scene3d_physicsInit")]
pub fn scene3d_physics_init(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    let gz = in_f64(input, &mut pos) as f32;
    let world_id = crate::physics3d::create_world(gx, gy, gz);
    let mut out = Vec::new();
    out_value_u64(&mut out, world_id as u64);
    out
}

/// scene3d_physicsDestroy(worldId)
#[wasm_bindgen(js_name = "scene3d_physicsDestroy")]
pub fn scene3d_physics_destroy(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    crate::terrain::remove_world(world_id);
    crate::physics3d::destroy_world(world_id);
    Vec::new()
}

/// scene3d_physicsSpawnBody(worldId, bodyId, data []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnBody")]
pub fn scene3d_physics_spawn_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let desc = crate::externs::physics3d::decode_body3d_desc(body_id, data);
    crate::physics3d::with_world(world_id, |world| world.spawn_body(&desc));
    Vec::new()
}

/// scene3d_physicsSpawnTrimeshBody(worldId, bodyId, modelId, data []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnTrimeshBody")]
pub fn scene3d_physics_spawn_trimesh_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let model_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let desc = crate::externs::physics3d::decode_trimesh_desc(body_id, data);
    let geometry = crate::externs::util::with_renderer_or_panic("physicsSpawnTrimeshBody", |r| {
        r.get_model_geometry(model_id)
    });
    let geometry = geometry
        .unwrap_or_else(|| panic!("physicsSpawnTrimeshBody: model not found: {}", model_id));
    crate::physics3d::with_world(world_id, |world| {
        world.spawn_trimesh_body(&desc, &geometry.positions, &geometry.indices)
    });
    Vec::new()
}

/// scene3d_physicsSpawnTrimeshBodyData(worldId, bodyId, data []byte, geometryData []byte)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnTrimeshBodyData")]
pub fn scene3d_physics_spawn_trimesh_body_data(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let data = in_bytes(input, &mut pos);
    let geometry_data = in_bytes(input, &mut pos);
    crate::externs::physics3d::spawn_trimesh_body_from_geometry(
        world_id,
        body_id,
        data,
        geometry_data,
    );
    Vec::new()
}

/// scene3d_physicsSpawnHeightfield(worldId, bodyId, heights []byte, rows, cols, sx, sy, sz, px, py, pz, layer, mask, friction, restitution)
#[wasm_bindgen(js_name = "scene3d_physicsSpawnHeightfield")]
pub fn scene3d_physics_spawn_heightfield(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let height_bytes = in_bytes(input, &mut pos);
    let rows = in_value(input, &mut pos) as u32;
    let cols = in_value(input, &mut pos) as u32;
    let scale_x = in_f64(input, &mut pos) as f32;
    let scale_y = in_f64(input, &mut pos) as f32;
    let scale_z = in_f64(input, &mut pos) as f32;
    let px = in_f64(input, &mut pos) as f32;
    let py = in_f64(input, &mut pos) as f32;
    let pz = in_f64(input, &mut pos) as f32;
    let layer = in_value(input, &mut pos) as u16;
    let mask = in_value(input, &mut pos) as u16;
    let friction = in_f64(input, &mut pos) as f32;
    let restitution = in_f64(input, &mut pos) as f32;

    let height_data: Vec<f32> = height_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let origin = Vec3::new(px, py, pz);
    let desc = crate::physics3d::HeightfieldDesc3D {
        body_id,
        pos: origin,
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
        origin,
        crate::terrain::TerrainData {
            model_id: 0,
            heights: height_data,
            rows,
            cols,
            scale_x,
            scale_y,
            scale_z,
            origin,
        },
    );
    Vec::new()
}

/// scene3d_physicsDestroyBody(worldId, bodyId)
#[wasm_bindgen(js_name = "scene3d_physicsDestroyBody")]
pub fn scene3d_physics_destroy_body(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    crate::terrain::remove_terrain(world_id, body_id);
    crate::physics3d::with_world(world_id, |world| world.destroy_body(body_id));
    Vec::new()
}

/// scene3d_physicsCreateRaycastVehicle(worldId, vehicleId, bodyId)
#[wasm_bindgen(js_name = "scene3d_physicsCreateRaycastVehicle")]
pub fn scene3d_physics_create_raycast_vehicle(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    crate::physics3d::with_world(world_id, |world| {
        world.create_raycast_vehicle(vehicle_id, body_id)
    });
    Vec::new()
}

/// scene3d_physicsDestroyRaycastVehicle(worldId, vehicleId)
#[wasm_bindgen(js_name = "scene3d_physicsDestroyRaycastVehicle")]
pub fn scene3d_physics_destroy_raycast_vehicle(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    crate::physics3d::with_world(world_id, |world| world.destroy_raycast_vehicle(vehicle_id));
    Vec::new()
}

/// scene3d_physicsAddRaycastVehicleWheel(worldId, vehicleId, desc []byte)
#[wasm_bindgen(js_name = "scene3d_physicsAddRaycastVehicleWheel")]
pub fn scene3d_physics_add_raycast_vehicle_wheel(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    let desc_bytes = in_bytes(input, &mut pos);
    let desc = crate::externs::physics3d::decode_raycast_vehicle_wheel_desc(desc_bytes);
    crate::physics3d::with_world(world_id, |world| {
        world.add_raycast_vehicle_wheel(vehicle_id, &desc)
    });
    Vec::new()
}

/// scene3d_physicsSetRaycastVehicleWheelControl(worldId, vehicleId, wheelId, steering, engineForce, brake)
#[wasm_bindgen(js_name = "scene3d_physicsSetRaycastVehicleWheelControl")]
pub fn scene3d_physics_set_raycast_vehicle_wheel_control(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    let wheel_id = in_value(input, &mut pos) as usize;
    let steering = in_f64(input, &mut pos) as f32;
    let engine_force = in_f64(input, &mut pos) as f32;
    let brake = in_f64(input, &mut pos) as f32;
    crate::physics3d::with_world(world_id, |world| {
        world.set_raycast_vehicle_wheel_control(vehicle_id, wheel_id, steering, engine_force, brake)
    });
    Vec::new()
}

/// scene3d_physicsApplyRaycastVehicleForces(worldId, vehicleId, fx, fy, fz, dragForce, downforce, waterLift, airControl, wallGrip, railGrip)
#[wasm_bindgen(js_name = "scene3d_physicsApplyRaycastVehicleForces")]
pub fn scene3d_physics_apply_raycast_vehicle_forces(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    let body_force = Vec3::new(
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
    );
    let drag_force = in_f64(input, &mut pos) as f32;
    let downforce = in_f64(input, &mut pos) as f32;
    let water_lift = in_f64(input, &mut pos) as f32;
    let air_control = in_f64(input, &mut pos) as f32;
    let wall_grip = in_f64(input, &mut pos) as f32;
    let rail_grip = in_f64(input, &mut pos) as f32;
    crate::physics3d::with_world(world_id, |world| {
        world.apply_raycast_vehicle_forces(
            vehicle_id,
            body_force,
            drag_force,
            downforce,
            water_lift,
            air_control,
            wall_grip,
            rail_grip,
        )
    });
    Vec::new()
}

/// scene3d_physicsSetBodyPose(worldId, bodyId, px, py, pz, qx, qy, qz, qw)
#[wasm_bindgen(js_name = "scene3d_physicsSetBodyPose")]
pub fn scene3d_physics_set_body_pose(input: &[u8]) -> Vec<u8> {
    use crate::math3d::{Quat, Vec3};
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let pos3 = Vec3::new(
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
    );
    let rot = Quat::new(
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
    );
    crate::physics3d::with_world(world_id, |world| world.set_body_pose(body_id, pos3, rot));
    Vec::new()
}

/// scene3d_physicsSetBodyMotion(worldId, bodyId, lvx, lvy, lvz, avx, avy, avz)
#[wasm_bindgen(js_name = "scene3d_physicsSetBodyMotion")]
pub fn scene3d_physics_set_body_motion(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let linear = Vec3::new(
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
    );
    let angular = Vec3::new(
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
        in_f64(input, &mut pos) as f32,
    );
    crate::physics3d::with_world(world_id, |world| {
        world.set_body_motion(body_id, linear, angular)
    });
    Vec::new()
}

/// scene3d_physicsSetBodySleepState(worldId, bodyId, sleeping)
#[wasm_bindgen(js_name = "scene3d_physicsSetBodySleepState")]
pub fn scene3d_physics_set_body_sleep_state(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let body_id = in_value(input, &mut pos) as u32;
    let sleeping = in_bool(input, &mut pos);
    crate::physics3d::with_world(world_id, |world| {
        world.set_body_sleep_state(body_id, sleeping)
    });
    Vec::new()
}

/// scene3d_physicsRaycastVehicleState(worldId, vehicleId) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsRaycastVehicleState")]
pub fn scene3d_physics_raycast_vehicle_state(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let vehicle_id = in_value(input, &mut pos) as u32;
    let state = crate::physics3d::with_world(world_id, |world| {
        world.serialize_raycast_vehicle_state(vehicle_id)
    });
    let mut out = Vec::new();
    out_bytes(&mut out, &state);
    out
}

/// scene3d_physicsRaycastVehicleStates(worldId) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsRaycastVehicleStates")]
pub fn scene3d_physics_raycast_vehicle_states(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let state =
        crate::physics3d::with_world(world_id, |world| world.serialize_raycast_vehicle_states());
    let mut out = Vec::new();
    out_bytes(&mut out, &state);
    out
}

/// scene3d_physicsStep(worldId, dt, cmds []byte) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsStep")]
pub fn scene3d_physics_step(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let dt = in_f64(input, &mut pos) as f32;
    let cmds = in_bytes(input, &mut pos).to_vec();
    let state = crate::physics3d::with_world(world_id, |world| {
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

/// scene3d_physicsSetGravity(worldId, gx, gy, gz)
#[wasm_bindgen(js_name = "scene3d_physicsSetGravity")]
pub fn scene3d_physics_set_gravity(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let gx = in_f64(input, &mut pos) as f32;
    let gy = in_f64(input, &mut pos) as f32;
    let gz = in_f64(input, &mut pos) as f32;
    crate::physics3d::with_world(world_id, |world| world.set_gravity(gx, gy, gz));
    Vec::new()
}

/// scene3d_physicsContacts(worldId) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsContacts")]
pub fn scene3d_physics_contacts(input: &[u8]) -> Vec<u8> {
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let contacts = crate::physics3d::with_world(world_id, |world| world.get_contacts());
    let buf = crate::physics3d::serialize_contacts_packet(&contacts);
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene3d_physicsRayCast(worldId, ox, oy, oz, dx, dy, dz, maxDist) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsRayCast")]
pub fn scene3d_physics_ray_cast(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let ox = in_f64(input, &mut pos) as f32;
    let oy = in_f64(input, &mut pos) as f32;
    let oz = in_f64(input, &mut pos) as f32;
    let dx = in_f64(input, &mut pos) as f32;
    let dy = in_f64(input, &mut pos) as f32;
    let dz = in_f64(input, &mut pos) as f32;
    let max_dist = in_f64(input, &mut pos) as f32;
    let origin = Vec3::new(ox, oy, oz);
    let dir = Vec3::new(dx, dy, dz);
    let result =
        crate::physics3d::with_world(world_id, |world| world.ray_cast(origin, dir, max_dist));
    let buf = match result {
        Some(hit) => {
            let mut b = Vec::with_capacity(61);
            b.push(1u8);
            b.extend_from_slice(&hit.body_id.to_le_bytes());
            b.extend_from_slice(&(hit.point.x as f64).to_le_bytes());
            b.extend_from_slice(&(hit.point.y as f64).to_le_bytes());
            b.extend_from_slice(&(hit.point.z as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.x as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.y as f64).to_le_bytes());
            b.extend_from_slice(&(hit.normal.z as f64).to_le_bytes());
            b.extend_from_slice(&(hit.toi as f64).to_le_bytes());
            b
        }
        None => vec![0u8],
    };
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}

/// scene3d_physicsQueryAABB(worldId, minX, minY, minZ, maxX, maxY, maxZ) → []byte
#[wasm_bindgen(js_name = "scene3d_physicsQueryAABB")]
pub fn scene3d_physics_query_aabb(input: &[u8]) -> Vec<u8> {
    use crate::math3d::Vec3;
    let mut pos = 0usize;
    let world_id = in_value(input, &mut pos) as u32;
    let min_x = in_f64(input, &mut pos) as f32;
    let min_y = in_f64(input, &mut pos) as f32;
    let min_z = in_f64(input, &mut pos) as f32;
    let max_x = in_f64(input, &mut pos) as f32;
    let max_y = in_f64(input, &mut pos) as f32;
    let max_z = in_f64(input, &mut pos) as f32;
    let min = Vec3::new(min_x, min_y, min_z);
    let max = Vec3::new(max_x, max_y, max_z);
    let ids = crate::physics3d::with_world(world_id, |world| world.query_aabb(min, max));
    let mut buf = Vec::with_capacity(4 + ids.len() * 4);
    buf.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in &ids {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    let mut out = Vec::new();
    out_bytes(&mut out, &buf);
    out
}
