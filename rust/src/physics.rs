//! 2D physics engine wrapper around Rapier2D.
//!
//! Manages a Rapier physics world with a handle registry mapping
//! Vo-side body IDs to Rapier RigidBodyHandles. Provides batch
//! command decoding and state serialization for cross-boundary sync.

use rapier2d::prelude::*;
use std::collections::HashMap;

/// Body type matching Vo's BodyType enum.
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum PhysBodyType {
    Dynamic = 0,
    Static = 1,
    Kinematic = 2,
}

/// Collider kind matching Vo's Collider.kind field.
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum ColliderKind {
    Box = 1,
    Circle = 2,
    Capsule = 3,
}

/// Description of a body to spawn, decoded from the extern call.
pub struct BodyDesc {
    pub body_id: u32,
    pub body_type: PhysBodyType,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub collider_kind: ColliderKind,
    pub collider_args: [f32; 3],
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub fixed_rotation: bool,
}

/// Physics command opcodes (sent from Vo to Rust each frame).
const CMD_APPLY_FORCE: u8 = 1;
const CMD_APPLY_IMPULSE: u8 = 2;
const CMD_SET_VELOCITY: u8 = 3;

/// Manages the Rapier2D physics world.
pub struct PhysicsWorld2D {
    // Rapier components
    gravity: Vector<f32>,
    integration_params: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,

    // Handle registry: Vo body_id -> Rapier handle
    handle_map: HashMap<u32, RigidBodyHandle>,
}

impl PhysicsWorld2D {
    /// Create a new physics world with the given gravity.
    pub fn new(gravity_x: f32, gravity_y: f32) -> Self {
        Self {
            gravity: vector![gravity_x, gravity_y],
            integration_params: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            handle_map: HashMap::new(),
        }
    }

    /// Set gravity.
    pub fn set_gravity(&mut self, x: f32, y: f32) {
        self.gravity = vector![x, y];
    }

    /// Spawn a rigid body with collider.
    pub fn spawn_body(&mut self, desc: &BodyDesc) {
        let rb = match desc.body_type {
            PhysBodyType::Dynamic => {
                let mut rb = RigidBodyBuilder::dynamic()
                    .translation(vector![desc.x, desc.y])
                    .rotation(desc.rotation)
                    .linear_damping(desc.linear_damping)
                    .build();
                if desc.fixed_rotation {
                    rb.lock_rotations(true, true);
                }
                rb
            }
            PhysBodyType::Static => {
                RigidBodyBuilder::fixed()
                    .translation(vector![desc.x, desc.y])
                    .rotation(desc.rotation)
                    .build()
            }
            PhysBodyType::Kinematic => {
                RigidBodyBuilder::kinematic_position_based()
                    .translation(vector![desc.x, desc.y])
                    .rotation(desc.rotation)
                    .build()
            }
        };

        let rb_handle = self.rigid_body_set.insert(rb);

        // Create collider
        let collider = match desc.collider_kind {
            ColliderKind::Box => {
                ColliderBuilder::cuboid(desc.collider_args[0], desc.collider_args[1])
                    .density(desc.density)
                    .friction(desc.friction)
                    .restitution(desc.restitution)
                    .build()
            }
            ColliderKind::Circle => {
                ColliderBuilder::ball(desc.collider_args[0])
                    .density(desc.density)
                    .friction(desc.friction)
                    .restitution(desc.restitution)
                    .build()
            }
            ColliderKind::Capsule => {
                ColliderBuilder::capsule_y(desc.collider_args[0], desc.collider_args[1])
                    .density(desc.density)
                    .friction(desc.friction)
                    .restitution(desc.restitution)
                    .build()
            }
        };

        self.collider_set.insert_with_parent(collider, rb_handle, &mut self.rigid_body_set);
        self.handle_map.insert(desc.body_id, rb_handle);
    }

    /// Destroy a body by Vo body_id.
    pub fn destroy_body(&mut self, body_id: u32) {
        if let Some(handle) = self.handle_map.remove(&body_id) {
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

    /// Apply buffered physics commands from the Vo side.
    /// Format: repeated [cmd_type: u8, body_id: u32, args...]
    ///   CMD_APPLY_FORCE:   body_id, fx, fy (2x f64)
    ///   CMD_APPLY_IMPULSE: body_id, ix, iy (2x f64)
    ///   CMD_SET_VELOCITY:  body_id, vx, vy (2x f64)
    pub fn apply_commands(&mut self, data: &[u8]) {
        let mut pos = 0;
        while pos < data.len() {
            let cmd = data[pos];
            pos += 1;

            if pos + 4 > data.len() { break; }
            let body_id = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
            pos += 4;

            let handle = match self.handle_map.get(&body_id) {
                Some(h) => *h,
                None => {
                    pos += 16; // skip 2x f64
                    continue;
                }
            };

            if pos + 16 > data.len() { break; }
            let v1 = f64::from_le_bytes([
                data[pos], data[pos+1], data[pos+2], data[pos+3],
                data[pos+4], data[pos+5], data[pos+6], data[pos+7],
            ]) as f32;
            pos += 8;
            let v2 = f64::from_le_bytes([
                data[pos], data[pos+1], data[pos+2], data[pos+3],
                data[pos+4], data[pos+5], data[pos+6], data[pos+7],
            ]) as f32;
            pos += 8;

            if let Some(rb) = self.rigid_body_set.get_mut(handle) {
                match cmd {
                    CMD_APPLY_FORCE => {
                        rb.add_force(vector![v1, v2], true);
                    }
                    CMD_APPLY_IMPULSE => {
                        rb.apply_impulse(vector![v1, v2], true);
                    }
                    CMD_SET_VELOCITY => {
                        rb.set_linvel(vector![v1, v2], true);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Step the physics world by dt seconds.
    pub fn step(&mut self, dt: f32) {
        self.integration_params.dt = dt;
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_params,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            None,
            &(),
            &(),
        );
    }

    /// Serialize all body positions into a byte buffer.
    /// Format: [count: u32, then for each body: body_id: u32, x: f64, y: f64, rotation: f64, vx: f64, vy: f64]
    pub fn serialize_state(&self) -> Vec<u8> {
        let count = self.handle_map.len() as u32;
        // 4 bytes count + 44 bytes per body (4 + 5*8)
        let mut buf = Vec::with_capacity(4 + self.handle_map.len() * 44);
        buf.extend_from_slice(&count.to_le_bytes());

        for (&body_id, &handle) in &self.handle_map {
            if let Some(rb) = self.rigid_body_set.get(handle) {
                let pos = rb.translation();
                let rot = rb.rotation().angle();
                let vel = rb.linvel();
                buf.extend_from_slice(&body_id.to_le_bytes());
                buf.extend_from_slice(&(pos.x as f64).to_le_bytes());
                buf.extend_from_slice(&(pos.y as f64).to_le_bytes());
                buf.extend_from_slice(&(rot as f64).to_le_bytes());
                buf.extend_from_slice(&(vel.x as f64).to_le_bytes());
                buf.extend_from_slice(&(vel.y as f64).to_le_bytes());
            }
        }

        buf
    }

    /// Get contact events from the narrow phase.
    /// Returns pairs of body IDs that are in contact.
    pub fn get_contacts(&self) -> Vec<(u32, u32)> {
        let mut contacts = Vec::new();
        // Build reverse map: collider handle -> body_id
        let mut collider_to_body: HashMap<ColliderHandle, u32> = HashMap::new();
        for (&body_id, &rb_handle) in &self.handle_map {
            if let Some(rb) = self.rigid_body_set.get(rb_handle) {
                for &col_handle in rb.colliders() {
                    collider_to_body.insert(col_handle, body_id);
                }
            }
        }

        self.narrow_phase.contact_pairs().for_each(|pair| {
            if pair.has_any_active_contact {
                if let (Some(&id_a), Some(&id_b)) = (
                    collider_to_body.get(&pair.collider1),
                    collider_to_body.get(&pair.collider2),
                ) {
                    contacts.push((id_a, id_b));
                }
            }
        });

        contacts
    }
}
