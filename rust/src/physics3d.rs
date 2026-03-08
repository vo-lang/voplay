//! 3D physics engine wrapper around Rapier3D.
//!
//! Mirrors the architecture of physics.rs (2D) but uses rapier3d types.
//! Manages rigid bodies, colliders, commands from Vo, state serialization,
//! and contact detection.

use std::collections::HashMap;
use std::sync::Mutex;
use rapier3d::prelude::*;
use rapier3d::na::{UnitQuaternion, Quaternion};

use crate::physics_registry::{WorldRegistry, PhysBodyType, with_world_in};

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
    Box3D,    // kind=4: args=[halfX, halfY, halfZ]
    Sphere,   // kind=5: args=[radius, 0, 0]
    Capsule,  // kind=3: args=[halfHeight, radius, 0]
}

/// Descriptor for spawning a 3D physics body.
pub struct BodyDesc3D {
    pub body_id: u32,
    pub body_type: PhysBodyType,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub qx: f32,
    pub qy: f32,
    pub qz: f32,
    pub qw: f32,
    pub collider_kind: ColliderKind3D,
    pub collider_args: [f32; 3],
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub fixed_rotation: bool,
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
        }
    }

    pub fn set_gravity(&mut self, gx: f32, gy: f32, gz: f32) {
        self.gravity = vector![gx, gy, gz];
    }

    /// Spawn a rigid body + collider from a descriptor.
    pub fn spawn_body(&mut self, desc: &BodyDesc3D) {
        let rb = match desc.body_type {
            PhysBodyType::Dynamic => {
                let mut rb = RigidBodyBuilder::dynamic()
                    .translation(vector![desc.x, desc.y, desc.z])
                    .rotation(
                        UnitQuaternion::from_quaternion(Quaternion::new(
                            desc.qw, desc.qx, desc.qy, desc.qz,
                        ))
                        .scaled_axis(),
                    )
                    .linear_damping(desc.linear_damping)
                    .build();
                if desc.fixed_rotation {
                    rb.lock_rotations(true, true);
                }
                rb
            }
            PhysBodyType::Static => RigidBodyBuilder::fixed()
                .translation(vector![desc.x, desc.y, desc.z])
                .rotation(
                    UnitQuaternion::from_quaternion(Quaternion::new(
                        desc.qw, desc.qx, desc.qy, desc.qz,
                    ))
                    .scaled_axis(),
                )
                .build(),
            PhysBodyType::Kinematic => RigidBodyBuilder::kinematic_position_based()
                .translation(vector![desc.x, desc.y, desc.z])
                .rotation(
                    UnitQuaternion::from_quaternion(Quaternion::new(
                        desc.qw, desc.qx, desc.qy, desc.qz,
                    ))
                    .scaled_axis(),
                )
                .build(),
        };

        let handle = self.rigid_body_set.insert(rb);

        let collider = match desc.collider_kind {
            ColliderKind3D::Box3D => {
                ColliderBuilder::cuboid(desc.collider_args[0], desc.collider_args[1], desc.collider_args[2])
            }
            ColliderKind3D::Sphere => {
                ColliderBuilder::ball(desc.collider_args[0])
            }
            ColliderKind3D::Capsule => {
                ColliderBuilder::capsule_y(desc.collider_args[0], desc.collider_args[1])
            }
        };

        let collider = collider
            .density(if desc.density > 0.0 { desc.density } else { 1.0 })
            .friction(desc.friction)
            .restitution(desc.restitution)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        self.collider_set
            .insert_with_parent(collider, handle, &mut self.rigid_body_set);

        self.handle_map.insert(desc.body_id, handle);
        self.reverse_map.insert(handle, desc.body_id);
    }

    /// Destroy a body by Vo ID.
    pub fn destroy_body(&mut self, body_id: u32) {
        if let Some(handle) = self.handle_map.remove(&body_id) {
            self.reverse_map.remove(&handle);
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

    /// Apply batch commands from Vo.
    /// Format per command: opcode(u8), body_id(u32 LE), x(f64 LE), y(f64 LE), z(f64 LE)
    pub fn apply_commands(&mut self, data: &[u8]) {
        let mut pos = 0;
        while pos + 29 <= data.len() {
            // 1 + 4 + 8 + 8 + 8 = 29 bytes per command
            let op = data[pos];
            pos += 1;
            let body_id = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
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
                1 => {
                    // ApplyForce
                    rb.add_force(vector![vx, vy, vz], true);
                }
                2 => {
                    // ApplyImpulse
                    rb.apply_impulse(vector![vx, vy, vz], true);
                }
                3 => {
                    // SetVelocity
                    rb.set_linvel(vector![vx, vy, vz], true);
                }
                4 => {
                    // SetPosition
                    rb.set_translation(vector![vx, vy, vz], true);
                }
                _ => {}
            }
        }
    }

    /// Step the physics world forward by dt seconds.
    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt;
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

        // 4 (count) + count * (4 + 11*8) = 4 + count * 92
        let mut buf = Vec::with_capacity(4 + count * 92);
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
    /// Returns the first hit: (body_id, hit_x, hit_y, hit_z, normal_x, normal_y, normal_z, toi).
    pub fn ray_cast(&self, ox: f32, oy: f32, oz: f32, dx: f32, dy: f32, dz: f32, max_dist: f32) -> Option<(u32, f32, f32, f32, f32, f32, f32, f32)> {
        let ray = Ray::new(point![ox, oy, oz], vector![dx, dy, dz]);
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
                    let hit_point = ray.point_at(intersection.time_of_impact);
                    return Some((
                        body_id,
                        hit_point.x, hit_point.y, hit_point.z,
                        intersection.normal.x, intersection.normal.y, intersection.normal.z,
                        intersection.time_of_impact,
                    ));
                }
            }
        }
        None
    }

    /// Query all bodies whose colliders intersect an AABB.
    /// Returns a list of body_ids.
    pub fn query_aabb(&self, min_x: f32, min_y: f32, min_z: f32, max_x: f32, max_y: f32, max_z: f32) -> Vec<u32> {
        let aabb = Aabb {
            mins: point![min_x, min_y, min_z],
            maxs: point![max_x, max_y, max_z],
        };

        let mut result = Vec::new();
        self.query_pipeline.colliders_with_aabb_intersecting_aabb(&aabb, |col_handle| {
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
                if let (Some(id1), Some(id2)) = (
                    self.reverse_map.get(&h1),
                    self.reverse_map.get(&h2),
                ) {
                    contacts.push((*id1, *id2));
                }
            }
        }
        contacts
    }
}
