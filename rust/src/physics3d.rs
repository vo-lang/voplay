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

const BODY_STATE_BYTES_3D: usize = 4 + 10 * 8;

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

    fn register_body(&mut self, body_id: u32, rb: RigidBody, collider: Collider) {
        let handle = self.rigid_body_set.insert(rb);
        self.collider_set
            .insert_with_parent(collider, handle, &mut self.rigid_body_set);
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

        self.register_body(desc.body_id, rb, collider);
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

        self.register_body(desc.body_id, rb, collider);
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

        self.register_body(desc.body_id, rb, collider);
    }

    /// Destroy a body by Vo ID.
    pub fn destroy_body(&mut self, body_id: u32) {
        if let Some(handle) = self.handle_map.remove(&body_id) {
            self.reverse_map.remove(&handle);
            self.raycast_vehicles
                .retain(|_, vehicle| vehicle.controller.chassis != handle);
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

    pub fn add_raycast_vehicle_wheel(
        &mut self,
        vehicle_id: u32,
        desc: &RaycastVehicleWheelDesc3D,
    ) {
        let vehicle = self.raycast_vehicles.get_mut(&vehicle_id).unwrap_or_else(|| {
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
        // suspension_length, steering, rotation (all f64)
        let mut buf = Vec::with_capacity(12 + wheels.len() * (1 + 12 * 8));
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
        }
        buf
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
                1..=4 => {
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
        }

        buf
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

    /// Return active collision pairs as (body_id_a, body_id_b).
    pub fn get_contacts(&self) -> Vec<(u32, u32)> {
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
                    contacts.push((*id1, *id2));
                }
            }
        }
        contacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(got: f32, want: f32, tol: f32) {
        assert!((got - want).abs() <= tol, "got {}, want {}", got, want);
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

    /// Build a Vec3 physics command (opcodes 1–4): 29 bytes.
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
        let state = world.serialize_state();
        assert!(state.len() >= 4);
        let count = u32::from_le_bytes([state[0], state[1], state[2], state[3]]);
        assert_eq!(count, 1);

        // Read position from serialized state: body_id(4) + pos(3×f64) = 28 bytes offset
        let pos_offset = 4 + 4; // count + body_id
        let px = f64::from_le_bytes(state[pos_offset..pos_offset + 8].try_into().unwrap()) as f32;
        assert_near(px, 10.0, 0.1);
    }

    #[test]
    fn serialize_state_uses_84_byte_body_records() {
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
            fixed_rotation: false,
            lock_rotation_x: false,
            lock_rotation_y: false,
            lock_rotation_z: false,
        };
        world.spawn_body(&desc);
        world.step(1.0 / 60.0);

        let state = world.serialize_state();
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

        assert_near(x, 1.0, 0.0001);
        assert_near(y, 2.0, 0.0001);
        assert_near(z, 3.0, 0.0001);
        assert_near(qw, 1.0, 0.0001);
        assert_near(vx, 0.0, 0.0001);
        assert_near(vy, 0.0, 0.0001);
        assert_near(vz, 0.0, 0.0001);
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
