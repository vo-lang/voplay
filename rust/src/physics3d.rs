//! 3D physics engine wrapper around Rapier3D.
//!
//! Mirrors the architecture of physics.rs (2D) but uses rapier3d types.
//! Manages rigid bodies, colliders, commands from Vo, state serialization,
//! and contact detection.

use rapier3d::control::{DynamicRayCastVehicleController, WheelTuning};
use rapier3d::na::{Quaternion, UnitQuaternion};
use rapier3d::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::math3d::{Quat, Vec3};
use crate::physics_registry::{with_world_in, PhysBodyType, WorldRegistry};

const BODY_STATE_BYTES_3D: usize = 4 + 13 * 8;
const SURFACE_MATERIAL_KIND_ROAD: u32 = 1;
const PHYSICS_BACKEND_PACKET_SCHEMA_VERSION: u32 = 1;
const PHYSICS_BACKEND_BODY_PACKET_KIND: u8 = 1;
const PHYSICS_BACKEND_CONTACT_PACKET_KIND: u8 = 2;
const PHYSICS_BACKEND_WHEEL_PACKET_KIND: u8 = 3;
const PHYSICS_BACKEND_PACKET_HEADER_BYTES: usize = 13;

fn physics_backend_packet_schema_hash(kind: u8, payload_len: usize) -> u32 {
    PHYSICS_BACKEND_PACKET_SCHEMA_VERSION
        .wrapping_mul(16_777_619)
        .wrapping_add((kind as u32).wrapping_mul(65_537))
        .wrapping_add(payload_len as u32)
}

fn serialize_backend_packet(kind: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(PHYSICS_BACKEND_PACKET_HEADER_BYTES + payload.len());
    packet.push(kind);
    packet.extend_from_slice(&PHYSICS_BACKEND_PACKET_SCHEMA_VERSION.to_le_bytes());
    packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    packet
        .extend_from_slice(&physics_backend_packet_schema_hash(kind, payload.len()).to_le_bytes());
    packet.extend_from_slice(&payload);
    packet
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceMaterial3D {
    pub id: [u8; 32],
    pub kind: u32,
}

impl Default for SurfaceMaterial3D {
    fn default() -> Self {
        Self {
            id: [0u8; 32],
            kind: SURFACE_MATERIAL_KIND_ROAD,
        }
    }
}

impl SurfaceMaterial3D {
    pub fn is_default(self) -> bool {
        self.id.iter().all(|b| *b == 0) && self.kind == SURFACE_MATERIAL_KIND_ROAD
    }
}

/// Global registry of 3D physics worlds, keyed by world handle.
static REGISTRY_3D: Mutex<Option<WorldRegistry<PhysicsWorld3D>>> = Mutex::new(None);

/// Create a new 3D physics world and return its handle.
pub fn create_world(gx: f32, gy: f32, gz: f32) -> u32 {
    let mut reg = REGISTRY_3D.lock().unwrap();
    let reg = reg.get_or_insert_with(WorldRegistry::new);
    reg.insert(PhysicsWorld3D::new(gx, gy, gz))
}

/// Destroy a 3D physics world by handle.
pub fn destroy_world(world_id: u32) {
    let mut reg = REGISTRY_3D.lock().unwrap();
    if let Some(reg) = reg.as_mut() {
        reg.remove(world_id);
    }
}

/// Access a 3D physics world by handle. Panics if not found.
pub fn with_world<R>(world_id: u32, f: impl FnOnce(&mut PhysicsWorld3D) -> R) -> R {
    with_world_in(&REGISTRY_3D, world_id, f)
}

/// Collider kind matching Vo's Collider.kind values for 3D.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColliderKind3D {
    Box3D,   // kind=4: args=[halfX, halfY, halfZ]
    Sphere,  // kind=5: args=[radius, 0, 0]
    Capsule, // kind=3: args=[halfHeight, radius, 0]
}

/// Descriptor for spawning a 3D physics body.
pub struct BodyDesc3D {
    pub body_id: u32,
    pub body_type: PhysBodyType,
    pub pos: Vec3,
    pub rot: Quat,
    pub collider_kind: ColliderKind3D,
    pub collider_args: [f32; 3],
    pub collider_offset: Vec3,
    pub layer: u16,
    pub mask: u16,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub surface_material: SurfaceMaterial3D,
    pub fixed_rotation: bool,
    pub lock_rotation_x: bool,
    pub lock_rotation_y: bool,
    pub lock_rotation_z: bool,
}

pub struct TrimeshDesc3D {
    pub body_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub layer: u16,
    pub mask: u16,
    pub friction: f32,
    pub restitution: f32,
}

pub struct HeightfieldDesc3D {
    pub body_id: u32,
    pub pos: Vec3,
    pub layer: u16,
    pub mask: u16,
    pub friction: f32,
    pub restitution: f32,
    pub rows: u32,
    pub cols: u32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub scale_z: f32,
}

pub struct RaycastVehicleWheelDesc3D {
    pub connection: Vec3,
    pub direction: Vec3,
    pub axle: Vec3,
    pub suspension_rest_length: f32,
    pub radius: f32,
    pub suspension_stiffness: f32,
    pub suspension_compression: f32,
    pub suspension_damping: f32,
    pub max_suspension_travel: f32,
    pub side_friction_stiffness: f32,
    pub friction_slip: f32,
    pub max_suspension_force: f32,
}

/// Result of a 3D ray cast query.
pub struct RayHit3D {
    pub body_id: u32,
    pub point: Vec3,
    pub normal: Vec3,
    pub toi: f32,
}

/// Detailed contact data exported from Rapier manifolds for Vo diagnostics.
pub struct ContactInfo3D {
    pub body1: u32,
    pub body2: u32,
    pub point: Vec3,
    pub normal: Vec3,
    pub relative_velocity: Vec3,
    pub relative_speed: f32,
    pub normal_impulse: f32,
    pub tangent_impulse: f32,
    pub has_impulse: bool,
}

struct RaycastVehicle3D {
    controller: DynamicRayCastVehicleController,
}

/// The 3D physics world wrapping Rapier3D components.
pub struct PhysicsWorld3D {
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    gravity: Vector<f32>,
    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    /// Map from Vo body ID → Rapier RigidBodyHandle.
    handle_map: HashMap<u32, RigidBodyHandle>,
    /// Reverse map from Rapier RigidBodyHandle → Vo body ID (for contact queries).
    reverse_map: HashMap<RigidBodyHandle, u32>,
    /// Surface metadata keyed by Rapier collider hit by raycast vehicle wheels.
    collider_surfaces: HashMap<ColliderHandle, SurfaceMaterial3D>,
    raycast_vehicles: HashMap<u32, RaycastVehicle3D>,
}

impl PhysicsWorld3D {
    pub fn new(gx: f32, gy: f32, gz: f32) -> Self {
        Self {
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            gravity: vector![gx, gy, gz],
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            handle_map: HashMap::new(),
            reverse_map: HashMap::new(),
            collider_surfaces: HashMap::new(),
            raycast_vehicles: HashMap::new(),
        }
    }

    pub fn set_gravity(&mut self, gx: f32, gy: f32, gz: f32) {
        self.gravity = vector![gx, gy, gz];
    }

    fn build_rigid_body(
        body_type: PhysBodyType,
        pos: Vec3,
        rot: Quat,
        linear_damping: f32,
        angular_damping: f32,
        fixed_rotation: bool,
        lock_rotation_x: bool,
        lock_rotation_y: bool,
        lock_rotation_z: bool,
    ) -> RigidBody {
        let translation = vector![pos.x, pos.y, pos.z];
        let rotation = UnitQuaternion::from_quaternion(Quaternion::new(rot.w, rot.x, rot.y, rot.z))
            .scaled_axis();

        match body_type {
            PhysBodyType::Dynamic => {
                let mut rb = RigidBodyBuilder::dynamic()
                    .translation(translation)
                    .rotation(rotation)
                    .linear_damping(linear_damping)
                    .angular_damping(angular_damping)
                    .build();
                if fixed_rotation {
                    rb.lock_rotations(true, true);
                } else if lock_rotation_x || lock_rotation_y || lock_rotation_z {
                    rb.set_enabled_rotations(
                        !lock_rotation_x,
                        !lock_rotation_y,
                        !lock_rotation_z,
                        true,
                    );
                }
                rb
            }
            PhysBodyType::Static => RigidBodyBuilder::fixed()
                .translation(translation)
                .rotation(rotation)
                .build(),
            PhysBodyType::Kinematic => RigidBodyBuilder::kinematic_position_based()
                .translation(translation)
                .rotation(rotation)
                .build(),
        }
    }

    fn register_body(
        &mut self,
        body_id: u32,
        rb: RigidBody,
        collider: Collider,
        surface: SurfaceMaterial3D,
    ) {
        let handle = self.rigid_body_set.insert(rb);
        let collider_handle =
            self.collider_set
                .insert_with_parent(collider, handle, &mut self.rigid_body_set);
        if !surface.is_default() {
            self.collider_surfaces.insert(collider_handle, surface);
        }
        self.handle_map.insert(body_id, handle);
        self.reverse_map.insert(handle, body_id);
    }

    /// Spawn a rigid body + collider from a descriptor.
    pub fn spawn_body(&mut self, desc: &BodyDesc3D) {
        let rb = Self::build_rigid_body(
            desc.body_type,
            desc.pos,
            desc.rot,
            desc.linear_damping,
            desc.angular_damping,
            desc.fixed_rotation,
            desc.lock_rotation_x,
            desc.lock_rotation_y,
            desc.lock_rotation_z,
        );

        let groups = InteractionGroups::new(
            Group::from_bits_truncate(desc.layer.into()),
            Group::from_bits_truncate(desc.mask.into()),
        );

        let collider = match desc.collider_kind {
            ColliderKind3D::Box3D => ColliderBuilder::cuboid(
                desc.collider_args[0],
                desc.collider_args[1],
                desc.collider_args[2],
            ),
            ColliderKind3D::Sphere => ColliderBuilder::ball(desc.collider_args[0]),
            ColliderKind3D::Capsule => {
                ColliderBuilder::capsule_y(desc.collider_args[0], desc.collider_args[1])
            }
        };

        let collider = collider
            .translation(vector![
                desc.collider_offset.x,
                desc.collider_offset.y,
                desc.collider_offset.z,
            ])
            .collision_groups(groups)
            .density(if desc.density > 0.0 {
                desc.density
            } else {
                1.0
            })
            .friction(desc.friction)
            .restitution(desc.restitution)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        self.register_body(desc.body_id, rb, collider, desc.surface_material);
    }

    /// Spawn a static rigid body with a triangle mesh collider.
    pub fn spawn_trimesh_body(
        &mut self,
        desc: &TrimeshDesc3D,
        positions: &[[f32; 3]],
        indices: &[u32],
    ) {
        assert!(
            !positions.is_empty(),
            "voplay: trimesh collider requires at least one vertex"
        );
        assert!(
            indices.len() >= 3 && indices.len() % 3 == 0,
            "voplay: trimesh collider indices must contain whole triangles"
        );

        let vertices: Vec<Point<f32>> = positions
            .iter()
            .map(|p| {
                point![
                    p[0] * desc.scale.x,
                    p[1] * desc.scale.y,
                    p[2] * desc.scale.z,
                ]
            })
            .collect();
        let triangles: Vec<[u32; 3]> = indices
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect();

        let rb = Self::build_rigid_body(
            PhysBodyType::Static,
            desc.pos,
            desc.rot,
            0.0,
            0.0,
            false,
            false,
            false,
            false,
        );
        let groups = InteractionGroups::new(
            Group::from_bits_truncate(desc.layer.into()),
            Group::from_bits_truncate(desc.mask.into()),
        );
        let collider = ColliderBuilder::trimesh(vertices, triangles)
            .collision_groups(groups)
            .friction(desc.friction)
            .restitution(desc.restitution)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        self.register_body(desc.body_id, rb, collider, SurfaceMaterial3D::default());
    }

    pub fn spawn_heightfield_body(&mut self, desc: &HeightfieldDesc3D, heights: &[f32]) {
        assert!(desc.rows >= 2, "voplay: heightfield rows must be >= 2");
        assert!(desc.cols >= 2, "voplay: heightfield cols must be >= 2");
        assert!(
            heights.len() == (desc.rows * desc.cols) as usize,
            "voplay: heightfield data size mismatch: got {}, want {}",
            heights.len(),
            desc.rows * desc.cols
        );
        assert!(
            desc.scale_x > 0.0,
            "voplay: heightfield scale_x must be > 0"
        );
        assert!(
            desc.scale_y > 0.0,
            "voplay: heightfield scale_y must be > 0"
        );
        assert!(
            desc.scale_z > 0.0,
            "voplay: heightfield scale_z must be > 0"
        );

        let matrix =
            rapier3d::na::DMatrix::from_fn(desc.rows as usize, desc.cols as usize, |r, c| {
                heights[r * desc.cols as usize + c] * desc.scale_y
            });
        let rb = Self::build_rigid_body(
            PhysBodyType::Static,
            desc.pos,
            Quat::IDENTITY,
            0.0,
            0.0,
            false,
            false,
            false,
            false,
        );
        let groups = InteractionGroups::new(
            Group::from_bits_truncate(desc.layer.into()),
            Group::from_bits_truncate(desc.mask.into()),
        );
        let collider =
            ColliderBuilder::heightfield(matrix, vector![desc.scale_x, 1.0, desc.scale_z])
                .collision_groups(groups)
                .friction(desc.friction)
                .restitution(desc.restitution)
                .active_events(ActiveEvents::COLLISION_EVENTS)
                .build();

        self.register_body(desc.body_id, rb, collider, SurfaceMaterial3D::default());
    }

    /// Destroy a body by Vo ID.
    pub fn destroy_body(&mut self, body_id: u32) {
        if let Some(handle) = self.handle_map.remove(&body_id) {
            self.reverse_map.remove(&handle);
            self.raycast_vehicles
                .retain(|_, vehicle| vehicle.controller.chassis != handle);
            if let Some(body) = self.rigid_body_set.get(handle) {
                let colliders: Vec<ColliderHandle> = body.colliders().to_vec();
                for collider in colliders {
                    self.collider_surfaces.remove(&collider);
                }
            }
            self.rigid_body_set.remove(
                handle,
                &mut self.island_manager,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                true,
            );
        }
    }

    pub fn create_raycast_vehicle(&mut self, vehicle_id: u32, body_id: u32) {
        let chassis = *self.handle_map.get(&body_id).unwrap_or_else(|| {
            panic!(
                "voplay: raycast vehicle chassis body not found: body_id={}",
                body_id
            )
        });
        let mut controller = DynamicRayCastVehicleController::new(chassis);
        controller.index_up_axis = 1;
        controller.index_forward_axis = 2;
        self.raycast_vehicles
            .insert(vehicle_id, RaycastVehicle3D { controller });
    }

    pub fn destroy_raycast_vehicle(&mut self, vehicle_id: u32) {
        self.raycast_vehicles.remove(&vehicle_id);
    }

    pub fn add_raycast_vehicle_wheel(&mut self, vehicle_id: u32, desc: &RaycastVehicleWheelDesc3D) {
        let vehicle = self
            .raycast_vehicles
            .get_mut(&vehicle_id)
            .unwrap_or_else(|| {
                panic!(
                    "voplay: raycast vehicle not found while adding wheel: {}",
                    vehicle_id
                )
            });
        let tuning = WheelTuning {
            suspension_stiffness: desc.suspension_stiffness,
            suspension_compression: desc.suspension_compression,
            suspension_damping: desc.suspension_damping,
            max_suspension_travel: desc.max_suspension_travel,
            side_friction_stiffness: desc.side_friction_stiffness,
            friction_slip: desc.friction_slip,
            max_suspension_force: desc.max_suspension_force,
        };
        vehicle.controller.add_wheel(
            point![desc.connection.x, desc.connection.y, desc.connection.z],
            vector![desc.direction.x, desc.direction.y, desc.direction.z],
            vector![desc.axle.x, desc.axle.y, desc.axle.z],
            desc.suspension_rest_length,
            desc.radius,
            &tuning,
        );
    }

    pub fn set_raycast_vehicle_wheel_control(
        &mut self,
        vehicle_id: u32,
        wheel_id: usize,
        steering: f32,
        engine_force: f32,
        brake: f32,
    ) {
        let Some(vehicle) = self.raycast_vehicles.get_mut(&vehicle_id) else {
            return;
        };
        if let Some(wheel) = vehicle.controller.wheels_mut().get_mut(wheel_id) {
            wheel.steering = steering;
            wheel.engine_force = engine_force;
            wheel.brake = brake;
        }
    }

    pub fn apply_raycast_vehicle_forces(
        &mut self,
        vehicle_id: u32,
        body_force: Vec3,
        drag_force: f32,
        downforce: f32,
        water_lift: f32,
        air_control: f32,
        wall_grip: f32,
        rail_grip: f32,
    ) {
        let Some(vehicle) = self.raycast_vehicles.get(&vehicle_id) else {
            return;
        };
        let Some(rb) = self.rigid_body_set.get_mut(vehicle.controller.chassis) else {
            return;
        };

        let vertical_force = water_lift - downforce;
        let mut force = vector![body_force.x, body_force.y, body_force.z];
        if force.y == 0.0 && vertical_force != 0.0 {
            force.y = vertical_force;
        }

        let vel = *rb.linvel();
        let speed = vel.norm();
        if drag_force > 0.0 && speed > 0.0 {
            force -= vel / speed * drag_force;
        }

        let grip = (wall_grip + rail_grip).clamp(0.0, 4.0);
        if grip > 0.0 && speed > 0.0 {
            force.x -= vel.x * grip * 0.25;
            force.z -= vel.z * grip * 0.25;
        }

        if force.norm_squared() > 0.0 {
            rb.add_force(force, true);
        }

        if air_control > 0.0 {
            let damping = (air_control * 0.02).clamp(0.0, 0.35);
            if damping > 0.0 {
                rb.set_angvel(*rb.angvel() * (1.0 - damping), true);
            }
        }
    }

    pub fn set_body_pose(&mut self, body_id: u32, pos: Vec3, rot: Quat) {
        let Some(handle) = self.handle_map.get(&body_id).copied() else {
            return;
        };
        let Some(rb) = self.rigid_body_set.get_mut(handle) else {
            return;
        };
        let rotation = UnitQuaternion::from_quaternion(Quaternion::new(rot.w, rot.x, rot.y, rot.z));
        rb.set_translation(vector![pos.x, pos.y, pos.z], true);
        rb.set_rotation(rotation, true);
    }

    pub fn set_body_motion(&mut self, body_id: u32, linear: Vec3, angular: Vec3) {
        let Some(handle) = self.handle_map.get(&body_id).copied() else {
            return;
        };
        let Some(rb) = self.rigid_body_set.get_mut(handle) else {
            return;
        };
        rb.set_linvel(vector![linear.x, linear.y, linear.z], true);
        rb.set_angvel(vector![angular.x, angular.y, angular.z], true);
    }

    pub fn set_body_sleep_state(&mut self, body_id: u32, sleeping: bool) {
        let Some(handle) = self.handle_map.get(&body_id).copied() else {
            return;
        };
        let Some(rb) = self.rigid_body_set.get_mut(handle) else {
            return;
        };
        if sleeping {
            rb.sleep();
        } else {
            rb.wake_up(true);
        }
    }

    fn update_raycast_vehicles(&mut self, dt: f32) {
        if self.raycast_vehicles.is_empty() {
            return;
        }
        self.query_pipeline.update(&self.collider_set);
        for vehicle in self.raycast_vehicles.values_mut() {
            let chassis = vehicle.controller.chassis;
            let filter = QueryFilter::default().exclude_rigid_body(chassis);
            vehicle.controller.update_vehicle(
                dt,
                &mut self.rigid_body_set,
                &self.collider_set,
                &self.query_pipeline,
                filter,
            );
        }
    }

    pub fn serialize_raycast_vehicle_state(&self, vehicle_id: u32) -> Vec<u8> {
        let Some(vehicle) = self.raycast_vehicles.get(&vehicle_id) else {
            return Vec::new();
        };
        let wheels = vehicle.controller.wheels();
        let mut speed = 0.0f32;
        if let Some(chassis) = self.rigid_body_set.get(vehicle.controller.chassis) {
            let forward = chassis.position() * vector![0.0, 0.0, -1.0];
            speed = forward.dot(chassis.linvel());
        }

        // speed(f64), wheel_count(u32), then per wheel:
        // contact(u8), center xyz, contact xyz, normal xyz,
        // suspension_length, steering, rotation (all f64),
        // material id (32 bytes, nul padded), material kind (u32).
        let mut buf = Vec::with_capacity(12 + wheels.len() * (1 + 12 * 8 + 32 + 4));
        buf.extend_from_slice(&(speed as f64).to_le_bytes());
        buf.extend_from_slice(&(wheels.len() as u32).to_le_bytes());
        for wheel in wheels {
            let info = wheel.raycast_info();
            let center = wheel.center();
            let contact = info.contact_point_ws;
            let normal = info.contact_normal_ws;
            buf.push(info.is_in_contact as u8);
            buf.extend_from_slice(&(center.x as f64).to_le_bytes());
            buf.extend_from_slice(&(center.y as f64).to_le_bytes());
            buf.extend_from_slice(&(center.z as f64).to_le_bytes());
            buf.extend_from_slice(&(contact.x as f64).to_le_bytes());
            buf.extend_from_slice(&(contact.y as f64).to_le_bytes());
            buf.extend_from_slice(&(contact.z as f64).to_le_bytes());
            buf.extend_from_slice(&(normal.x as f64).to_le_bytes());
            buf.extend_from_slice(&(normal.y as f64).to_le_bytes());
            buf.extend_from_slice(&(normal.z as f64).to_le_bytes());
            buf.extend_from_slice(&(info.suspension_length as f64).to_le_bytes());
            buf.extend_from_slice(&(wheel.steering as f64).to_le_bytes());
            buf.extend_from_slice(&(wheel.rotation as f64).to_le_bytes());
            let material = info
                .ground_object
                .and_then(|handle| self.collider_surfaces.get(&handle).copied())
                .unwrap_or_default();
            buf.extend_from_slice(&material.id);
            buf.extend_from_slice(&material.kind.to_le_bytes());
        }
        serialize_backend_packet(PHYSICS_BACKEND_WHEEL_PACKET_KIND, buf)
    }

    /// Apply batch commands from Vo.
    ///
    /// Vec3 commands (opcodes 1–4): opcode(u8), body_id(u32 LE), x(f64), y(f64), z(f64) = 29 bytes
    /// Quat commands (opcode 5):    opcode(u8), body_id(u32 LE), qx(f64), qy(f64), qz(f64), qw(f64) = 37 bytes
    pub fn apply_commands(&mut self, data: &[u8]) {
        let mut pos = 0;
        while pos < data.len() {
            // Every command starts with opcode(1) + body_id(4) = 5 bytes header
            assert!(
                pos + 5 <= data.len(),
                "voplay: physics command stream truncated at header (pos={}, len={})",
                pos,
                data.len()
            );
            let op = data[pos];
            pos += 1;
            let body_id =
                u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;

            match op {
                // Vec3 commands: 3 × f64 = 24 bytes payload
                1..=4 | 6 => {
                    assert!(
                        pos + 24 <= data.len(),
                        "voplay: physics Vec3 command truncated (op={}, pos={}, len={})",
                        op,
                        pos,
                        data.len()
                    );
                    let vx = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;
                    let vy = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;
                    let vz = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;

                    let handle = match self.handle_map.get(&body_id) {
                        Some(h) => *h,
                        None => continue,
                    };
                    let rb = match self.rigid_body_set.get_mut(handle) {
                        Some(rb) => rb,
                        None => continue,
                    };

                    match op {
                        1 => rb.add_force(vector![vx, vy, vz], true),
                        2 => rb.apply_impulse(vector![vx, vy, vz], true),
                        3 => rb.set_linvel(vector![vx, vy, vz], true),
                        4 => rb.set_translation(vector![vx, vy, vz], true),
                        6 => rb.set_angvel(vector![vx, vy, vz], true),
                        _ => unreachable!(),
                    }
                }
                // Quat command (SetRotation): 4 × f64 = 32 bytes payload
                5 => {
                    assert!(
                        pos + 32 <= data.len(),
                        "voplay: physics Quat command truncated (pos={}, len={})",
                        pos,
                        data.len()
                    );
                    let qx = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;
                    let qy = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;
                    let qz = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;
                    let qw = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as f32;
                    pos += 8;

                    let handle = match self.handle_map.get(&body_id) {
                        Some(h) => *h,
                        None => continue,
                    };
                    let rb = match self.rigid_body_set.get_mut(handle) {
                        Some(rb) => rb,
                        None => continue,
                    };
                    let rotation = UnitQuaternion::from_quaternion(Quaternion::new(qw, qx, qy, qz));
                    rb.set_rotation(rotation, true);
                }
                _ => panic!(
                    "voplay: unknown physics command opcode {} at pos {}",
                    op,
                    pos - 5
                ),
            }
        }
    }

    /// Step the physics world forward by dt seconds.
    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt;
        self.update_raycast_vehicles(dt);
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &(),
            &(),
        );
    }

    /// Serialize all dynamic/kinematic body states.
    /// Format: count(u32 LE), then per body:
    ///   body_id(u32 LE), x(f64), y(f64), z(f64),
    ///   qx(f64), qy(f64), qz(f64), qw(f64),
    ///   vx(f64), vy(f64), vz(f64)
    pub fn serialize_state(&self) -> Vec<u8> {
        // Count non-fixed bodies
        let count = self
            .handle_map
            .iter()
            .filter(|(_, h)| {
                self.rigid_body_set
                    .get(**h)
                    .map(|rb| !rb.is_fixed())
                    .unwrap_or(false)
            })
            .count();

        // 4 (count) + count * (4 + 10*8)
        let mut buf = Vec::with_capacity(4 + count * BODY_STATE_BYTES_3D);
        buf.extend_from_slice(&(count as u32).to_le_bytes());

        for (body_id, handle) in &self.handle_map {
            let rb = match self.rigid_body_set.get(*handle) {
                Some(rb) => rb,
                None => continue,
            };
            if rb.is_fixed() {
                continue;
            }

            let pos = rb.translation();
            let rot = rb.rotation();
            let vel = rb.linvel();
            let angvel = rb.angvel();

            buf.extend_from_slice(&body_id.to_le_bytes());
            buf.extend_from_slice(&(pos.x as f64).to_le_bytes());
            buf.extend_from_slice(&(pos.y as f64).to_le_bytes());
            buf.extend_from_slice(&(pos.z as f64).to_le_bytes());
            buf.extend_from_slice(&(rot.i as f64).to_le_bytes());
            buf.extend_from_slice(&(rot.j as f64).to_le_bytes());
            buf.extend_from_slice(&(rot.k as f64).to_le_bytes());
            buf.extend_from_slice(&(rot.w as f64).to_le_bytes());
            buf.extend_from_slice(&(vel.x as f64).to_le_bytes());
            buf.extend_from_slice(&(vel.y as f64).to_le_bytes());
            buf.extend_from_slice(&(vel.z as f64).to_le_bytes());
            buf.extend_from_slice(&(angvel.x as f64).to_le_bytes());
            buf.extend_from_slice(&(angvel.y as f64).to_le_bytes());
            buf.extend_from_slice(&(angvel.z as f64).to_le_bytes());
        }

        serialize_backend_packet(PHYSICS_BACKEND_BODY_PACKET_KIND, buf)
    }

    /// Ray cast into the 3D physics world.
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit3D> {
        let ray = Ray::new(
            point![origin.x, origin.y, origin.z],
            vector![dir.x, dir.y, dir.z],
        );
        let filter = QueryFilter::default();

        if let Some((col_handle, intersection)) = self.query_pipeline.cast_ray_and_get_normal(
            &self.rigid_body_set,
            &self.collider_set,
            &ray,
            max_dist,
            true,
            filter,
        ) {
            let rb_handle = self.collider_set.get(col_handle).and_then(|c| c.parent());
            if let Some(h) = rb_handle {
                if let Some(&body_id) = self.reverse_map.get(&h) {
                    let hp = ray.point_at(intersection.time_of_impact);
                    let n = &intersection.normal;
                    return Some(RayHit3D {
                        body_id,
                        point: Vec3::new(hp.x, hp.y, hp.z),
                        normal: Vec3::new(n.x, n.y, n.z),
                        toi: intersection.time_of_impact,
                    });
                }
            }
        }
        None
    }

    /// Query all bodies whose colliders intersect an AABB.
    pub fn query_aabb(&self, min: Vec3, max: Vec3) -> Vec<u32> {
        let aabb = Aabb {
            mins: point![min.x, min.y, min.z],
            maxs: point![max.x, max.y, max.z],
        };

        let mut result = Vec::new();
        self.query_pipeline
            .colliders_with_aabb_intersecting_aabb(&aabb, |col_handle| {
                let rb_handle = self.collider_set.get(*col_handle).and_then(|c| c.parent());
                if let Some(h) = rb_handle {
                    if let Some(&body_id) = self.reverse_map.get(&h) {
                        if !result.contains(&body_id) {
                            result.push(body_id);
                        }
                    }
                }
                true // continue iterating
            });
        result
    }

    /// Return active collision contacts with manifold-derived diagnostics.
    pub fn get_contacts(&self) -> Vec<ContactInfo3D> {
        let mut contacts = Vec::new();
        for pair in self.narrow_phase.contact_pairs() {
            if !pair.has_any_active_contact {
                continue;
            }
            let c1 = pair.collider1;
            let c2 = pair.collider2;
            let rb1 = self.collider_set.get(c1).and_then(|c| c.parent());
            let rb2 = self.collider_set.get(c2).and_then(|c| c.parent());
            if let (Some(h1), Some(h2)) = (rb1, rb2) {
                if let (Some(id1), Some(id2)) =
                    (self.reverse_map.get(&h1), self.reverse_map.get(&h2))
                {
                    let vel1 = self
                        .rigid_body_set
                        .get(h1)
                        .map(|body| *body.linvel())
                        .unwrap_or_else(Vector::zeros);
                    let vel2 = self
                        .rigid_body_set
                        .get(h2)
                        .map(|body| *body.linvel())
                        .unwrap_or_else(Vector::zeros);
                    let relative_velocity = vel2 - vel1;
                    let (normal_impulse, normal_from_impulse) = pair.max_impulse();
                    let mut normal = normal_from_impulse;
                    if normal.norm_squared() <= f32::EPSILON {
                        if let Some(manifold) = pair.manifolds.first() {
                            normal = manifold.data.normal;
                        }
                    }
                    if normal.norm_squared() <= f32::EPSILON {
                        normal = vector![0.0, 1.0, 0.0];
                    } else {
                        normal = normal.normalize();
                    }
                    let mut point = None;
                    let mut tangent_impulse = 0.0f32;
                    for manifold in &pair.manifolds {
                        for solver_contact in &manifold.data.solver_contacts {
                            if point.is_none() {
                                point = Some(solver_contact.point);
                            }
                            tangent_impulse += solver_contact.warmstart_tangent_impulse.norm();
                        }
                    }
                    if point.is_none() {
                        if let Some((manifold, contact)) = pair.find_deepest_contact() {
                            let world_p1 = self
                                .collider_set
                                .get(c1)
                                .map(|collider| collider.position() * contact.local_p1);
                            let world_p2 = self
                                .collider_set
                                .get(c2)
                                .map(|collider| collider.position() * contact.local_p2);
                            point = match (world_p1, world_p2) {
                                (Some(p1), Some(p2)) => Some(p1 + (p2 - p1) * 0.5),
                                (Some(p1), None) => Some(p1),
                                (None, Some(p2)) => Some(p2),
                                _ => manifold.data.solver_contacts.first().map(|c| c.point),
                            };
                        }
                    }
                    let point = point.unwrap_or_else(Point::origin);
                    contacts.push(ContactInfo3D {
                        body1: *id1,
                        body2: *id2,
                        point: Vec3::new(point.x, point.y, point.z),
                        normal: Vec3::new(normal.x, normal.y, normal.z),
                        relative_velocity: Vec3::new(
                            relative_velocity.x,
                            relative_velocity.y,
                            relative_velocity.z,
                        ),
                        relative_speed: relative_velocity.norm(),
                        normal_impulse,
                        tangent_impulse,
                        has_impulse: normal_impulse.abs() > 0.0 || tangent_impulse.abs() > 0.0,
                    });
                }
            }
        }
        contacts
    }
}

pub fn serialize_contacts_packet(contacts: &[ContactInfo3D]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + contacts.len() * (8 + 12 * 8 + 1));
    payload.extend_from_slice(&(contacts.len() as u32).to_le_bytes());
    for contact in contacts {
        payload.extend_from_slice(&contact.body1.to_le_bytes());
        payload.extend_from_slice(&contact.body2.to_le_bytes());
        payload.extend_from_slice(&(contact.point.x as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.point.y as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.point.z as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.normal.x as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.normal.y as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.normal.z as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.relative_velocity.x as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.relative_velocity.y as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.relative_velocity.z as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.relative_speed as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.normal_impulse as f64).to_le_bytes());
        payload.extend_from_slice(&(contact.tangent_impulse as f64).to_le_bytes());
        payload.push(contact.has_impulse as u8);
    }
    serialize_backend_packet(PHYSICS_BACKEND_CONTACT_PACKET_KIND, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(got: f32, want: f32, tol: f32) {
        assert!((got - want).abs() <= tol, "got {}, want {}", got, want);
    }

    fn packet_payload(packet: &[u8], kind: u8) -> &[u8] {
        assert_eq!(packet[0], kind);
        let version = u32::from_le_bytes(packet[1..5].try_into().unwrap());
        assert_eq!(version, PHYSICS_BACKEND_PACKET_SCHEMA_VERSION);
        let payload_len = u32::from_le_bytes(packet[5..9].try_into().unwrap()) as usize;
        let schema_hash = u32::from_le_bytes(packet[9..13].try_into().unwrap());
        assert_eq!(
            schema_hash,
            physics_backend_packet_schema_hash(kind, payload_len)
        );
        assert_eq!(
            packet.len(),
            PHYSICS_BACKEND_PACKET_HEADER_BYTES + payload_len
        );
        &packet[PHYSICS_BACKEND_PACKET_HEADER_BYTES..]
    }

    #[test]
    fn spawn_body_applies_local_collider_offset() {
        let mut world = PhysicsWorld3D::new(0.0, -9.8, 0.0);
        let desc = BodyDesc3D {
            body_id: 3,
            body_type: PhysBodyType::Static,
            pos: Vec3::new(10.0, 0.0, 0.0),
            rot: Quat::IDENTITY,
            collider_kind: ColliderKind3D::Box3D,
            collider_args: [1.0, 1.0, 1.0],
            collider_offset: Vec3::new(2.0, 0.0, 0.0),
            layer: 1,
            mask: 0xFFFF,
            density: 0.0,
            friction: 0.5,
            restitution: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            surface_material: SurfaceMaterial3D::default(),
            fixed_rotation: false,
            lock_rotation_x: false,
            lock_rotation_y: false,
            lock_rotation_z: false,
        };

        world.spawn_body(&desc);
        world.step(1.0 / 60.0);

        let hit = world
            .ray_cast(Vec3::new(12.0, 3.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 10.0)
            .expect("expected ray cast hit on offset collider");
        assert_eq!(hit.body_id, 3);
        assert_near(hit.point.x, 12.0, 0.0001);
        assert_near(hit.point.y, 1.0, 0.0001);
        assert_near(hit.point.z, 0.0, 0.0001);
    }

    #[test]
    fn spawn_trimesh_body_bakes_scale_and_supports_queries() {
        let mut world = PhysicsWorld3D::new(0.0, -9.8, 0.0);
        let desc = TrimeshDesc3D {
            body_id: 7,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::new(2.0, 1.0, 3.0),
            layer: 1,
            mask: 0xFFFF,
            friction: 0.5,
            restitution: 0.0,
        };
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];

        world.spawn_trimesh_body(&desc, &positions, &indices);
        world.step(1.0 / 60.0);

        let hit = world
            .ray_cast(Vec3::new(1.5, 1.0, 2.5), Vec3::new(0.0, -1.0, 0.0), 5.0)
            .expect("expected ray cast hit on trimesh");
        assert_eq!(hit.body_id, 7);
        assert_near(hit.point.x, 1.5, 0.0001);
        assert_near(hit.point.y, 0.0, 0.0001);
        assert_near(hit.point.z, 2.5, 0.0001);
        assert_near(hit.toi, 1.0, 0.0001);
        assert_near(hit.normal.x, 0.0, 0.0001);
        assert!(hit.normal.y.abs() > 0.999);
        assert_near(hit.normal.z, 0.0, 0.0001);

        let ids = world.query_aabb(Vec3::new(-0.1, -0.1, -0.1), Vec3::new(2.1, 0.1, 3.1));
        assert_eq!(ids, vec![7]);
    }

    #[test]
    fn spawn_heightfield_body_supports_ray_cast_and_aabb() {
        let mut world = PhysicsWorld3D::new(0.0, -9.8, 0.0);
        let desc = HeightfieldDesc3D {
            body_id: 11,
            pos: Vec3::new(5.0, 2.0, -3.0),
            layer: 1,
            mask: 0xFFFF,
            friction: 0.8,
            restitution: 0.0,
            rows: 2,
            cols: 2,
            scale_x: 4.0,
            scale_y: 10.0,
            scale_z: 6.0,
        };
        let heights = vec![0.0, 0.5, 1.0, 0.25];

        world.spawn_heightfield_body(&desc, &heights);
        world.step(1.0 / 60.0);

        let hit = world
            .ray_cast(Vec3::new(5.0, 20.0, -3.0), Vec3::new(0.0, -1.0, 0.0), 30.0)
            .expect("expected ray cast hit on heightfield");
        assert_eq!(hit.body_id, 11);
        assert!(
            hit.point.y >= 2.0,
            "heightfield hit should include world translation"
        );

        let ids = world.query_aabb(Vec3::new(2.9, 1.9, -6.1), Vec3::new(7.1, 12.1, 0.1));
        assert_eq!(ids, vec![11]);
    }

    /// Build a Vec3 physics command (opcodes 1–4 and 6): 29 bytes.
    fn build_vec3_cmd(op: u8, body_id: u32, x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(29);
        buf.push(op);
        buf.extend_from_slice(&body_id.to_le_bytes());
        buf.extend_from_slice(&x.to_le_bytes());
        buf.extend_from_slice(&y.to_le_bytes());
        buf.extend_from_slice(&z.to_le_bytes());
        buf
    }

    /// Build a Quat physics command (opcode 5): 37 bytes.
    fn build_quat_cmd(body_id: u32, qx: f64, qy: f64, qz: f64, qw: f64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(37);
        buf.push(5);
        buf.extend_from_slice(&body_id.to_le_bytes());
        buf.extend_from_slice(&qx.to_le_bytes());
        buf.extend_from_slice(&qy.to_le_bytes());
        buf.extend_from_slice(&qz.to_le_bytes());
        buf.extend_from_slice(&qw.to_le_bytes());
        buf
    }

    #[test]
    fn apply_commands_set_rotation_does_not_corrupt_stream() {
        let mut world = PhysicsWorld3D::new(0.0, 0.0, 0.0);

        // Spawn a kinematic box at origin
        let desc = BodyDesc3D {
            body_id: 1,
            body_type: PhysBodyType::Kinematic,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            collider_kind: ColliderKind3D::Box3D,
            collider_args: [1.0, 1.0, 1.0],
            collider_offset: Vec3::ZERO,
            layer: 1,
            mask: 0xFFFF,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            surface_material: SurfaceMaterial3D::default(),
            fixed_rotation: false,
            lock_rotation_x: false,
            lock_rotation_y: false,
            lock_rotation_z: false,
        };
        world.spawn_body(&desc);

        // Build a command buffer with mixed Vec3 and Quat commands:
        //   SetPosition (op=4, 29 bytes) → move to (5, 0, 0)
        //   SetRotation (op=5, 37 bytes) → 90° around Y axis
        //   SetPosition (op=4, 29 bytes) → move to (10, 0, 0)
        // If the parser still assumed fixed 29 bytes, the Quat command would
        // corrupt the second SetPosition.
        let half_sqrt2 = std::f64::consts::FRAC_1_SQRT_2;
        let mut cmds = Vec::new();
        cmds.extend_from_slice(&build_vec3_cmd(4, 1, 5.0, 0.0, 0.0));
        cmds.extend_from_slice(&build_quat_cmd(1, 0.0, half_sqrt2, 0.0, half_sqrt2));
        cmds.extend_from_slice(&build_vec3_cmd(4, 1, 10.0, 0.0, 0.0));

        // Total: 29 + 37 + 29 = 95 bytes
        assert_eq!(cmds.len(), 95);

        world.apply_commands(&cmds);
        world.step(1.0 / 60.0);

        // After commands, position should be (10, 0, 0) — the last SetPosition wins.
        let state_packet = world.serialize_state();
        let state = packet_payload(&state_packet, PHYSICS_BACKEND_BODY_PACKET_KIND);
        assert!(state.len() >= 4);
        let count = u32::from_le_bytes([state[0], state[1], state[2], state[3]]);
        assert_eq!(count, 1);

        // Read position from serialized state: body_id(4) + pos(3×f64) = 28 bytes offset
        let pos_offset = 4 + 4; // count + body_id
        let px = f64::from_le_bytes(state[pos_offset..pos_offset + 8].try_into().unwrap()) as f32;
        assert_near(px, 10.0, 0.1);
    }

    #[test]
    fn serialize_state_uses_108_byte_body_records() {
        let mut world = PhysicsWorld3D::new(0.0, 0.0, 0.0);
        let desc = BodyDesc3D {
            body_id: 42,
            body_type: PhysBodyType::Kinematic,
            pos: Vec3::new(1.0, 2.0, 3.0),
            rot: Quat::IDENTITY,
            collider_kind: ColliderKind3D::Box3D,
            collider_args: [0.5, 0.5, 0.5],
            collider_offset: Vec3::ZERO,
            layer: 1,
            mask: 0xFFFF,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            surface_material: SurfaceMaterial3D::default(),
            fixed_rotation: false,
            lock_rotation_x: false,
            lock_rotation_y: false,
            lock_rotation_z: false,
        };
        world.spawn_body(&desc);
        world.step(1.0 / 60.0);

        let state_packet = world.serialize_state();
        let state = packet_payload(&state_packet, PHYSICS_BACKEND_BODY_PACKET_KIND);
        assert_eq!(state.len(), 4 + BODY_STATE_BYTES_3D);

        let count = u32::from_le_bytes(state[0..4].try_into().unwrap());
        assert_eq!(count, 1);

        let body_id = u32::from_le_bytes(state[4..8].try_into().unwrap());
        assert_eq!(body_id, 42);

        let x = f64::from_le_bytes(state[8..16].try_into().unwrap()) as f32;
        let y = f64::from_le_bytes(state[16..24].try_into().unwrap()) as f32;
        let z = f64::from_le_bytes(state[24..32].try_into().unwrap()) as f32;
        let qw = f64::from_le_bytes(state[56..64].try_into().unwrap()) as f32;
        let vx = f64::from_le_bytes(state[64..72].try_into().unwrap()) as f32;
        let vy = f64::from_le_bytes(state[72..80].try_into().unwrap()) as f32;
        let vz = f64::from_le_bytes(state[80..88].try_into().unwrap()) as f32;
        let avx = f64::from_le_bytes(state[88..96].try_into().unwrap()) as f32;
        let avy = f64::from_le_bytes(state[96..104].try_into().unwrap()) as f32;
        let avz = f64::from_le_bytes(state[104..112].try_into().unwrap()) as f32;

        assert_near(x, 1.0, 0.0001);
        assert_near(y, 2.0, 0.0001);
        assert_near(z, 3.0, 0.0001);
        assert_near(qw, 1.0, 0.0001);
        assert_near(vx, 0.0, 0.0001);
        assert_near(vy, 0.0, 0.0001);
        assert_near(vz, 0.0, 0.0001);
        assert_near(avx, 0.0, 0.0001);
        assert_near(avy, 0.0, 0.0001);
        assert_near(avz, 0.0, 0.0001);
    }

    #[test]
    fn apply_commands_sets_angular_velocity() {
        let mut world = PhysicsWorld3D::new(0.0, 0.0, 0.0);
        let desc = BodyDesc3D {
            body_id: 7,
            body_type: PhysBodyType::Dynamic,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            collider_kind: ColliderKind3D::Box3D,
            collider_args: [0.5, 0.5, 0.5],
            collider_offset: Vec3::ZERO,
            layer: 1,
            mask: 0xFFFF,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            surface_material: SurfaceMaterial3D::default(),
            fixed_rotation: false,
            lock_rotation_x: false,
            lock_rotation_y: false,
            lock_rotation_z: false,
        };
        world.spawn_body(&desc);
        world.apply_commands(&build_vec3_cmd(6, 7, 1.0, 2.0, 3.0));

        let handle = *world.handle_map.get(&7).unwrap();
        let rb = world.rigid_body_set.get(handle).unwrap();
        let angvel = rb.angvel();
        assert_near(angvel.x, 1.0, 0.0001);
        assert_near(angvel.y, 2.0, 0.0001);
        assert_near(angvel.z, 3.0, 0.0001);
    }

    #[test]
    #[should_panic(expected = "unknown physics command opcode")]
    fn apply_commands_panics_on_unknown_opcode() {
        let mut world = PhysicsWorld3D::new(0.0, 0.0, 0.0);
        // opcode 99 is invalid
        let cmds = build_vec3_cmd(99, 1, 0.0, 0.0, 0.0);
        world.apply_commands(&cmds);
    }
}
