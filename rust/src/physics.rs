//! 2D physics engine wrapper around Rapier2D.
//!
//! Manages a Rapier physics world with a handle registry mapping
//! Vo-side body IDs to Rapier RigidBodyHandles. Provides batch
//! command decoding and state serialization for cross-boundary sync.

use rapier2d::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::physics_registry::{WorldRegistry, PhysBodyType, with_world_in};

/// Global registry of 2D physics worlds, keyed by world handle.
static REGISTRY: Mutex<Option<WorldRegistry<PhysicsWorld2D>>> = Mutex::new(None);

/// Create a new 2D physics world and return its handle.
pub fn create_world(gravity_x: f32, gravity_y: f32) -> u32 {
    let mut reg = REGISTRY.lock().unwrap();
    let reg = reg.get_or_insert_with(WorldRegistry::new);
    reg.insert(PhysicsWorld2D::new(gravity_x, gravity_y))
}

/// Destroy a 2D physics world by handle.
pub fn destroy_world(world_id: u32) {
    let mut reg = REGISTRY.lock().unwrap();
    if let Some(reg) = reg.as_mut() {
        reg.remove(world_id);
    }
}

/// Access a physics world by handle. Panics if not found.
pub fn with_world<R>(world_id: u32, f: impl FnOnce(&mut PhysicsWorld2D) -> R) -> R {
    with_world_in(&REGISTRY, world_id, f)
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
const CMD_SET_POSITION: u8 = 4;

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
    /// Reverse map from Rapier RigidBodyHandle → Vo body ID (for contact/query).
    reverse_map: HashMap<RigidBodyHandle, u32>,
    query_pipeline: QueryPipeline,
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
            reverse_map: HashMap::new(),
            query_pipeline: QueryPipeline::new(),
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
        self.reverse_map.insert(rb_handle, desc.body_id);
    }

    /// Destroy a body by Vo body_id.
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

    /// Apply buffered physics commands from the Vo side.
    /// Format: repeated [cmd_type: u8, body_id: u32, args...]
    ///   CMD_APPLY_FORCE:   body_id, fx, fy (2x f64)
    ///   CMD_APPLY_IMPULSE: body_id, ix, iy (2x f64)
    ///   CMD_SET_VELOCITY:  body_id, vx, vy (2x f64)
    ///   CMD_SET_POSITION:  body_id, x, y (2x f64)
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
                    CMD_SET_POSITION => {
                        rb.set_translation(vector![v1, v2], true);
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
            Some(&mut self.query_pipeline),
            &(),
            &(),
        );
    }

    /// Serialize non-fixed body positions into a byte buffer.
    /// Format: [count: u32, then for each body: body_id: u32, x: f64, y: f64, rotation: f64, vx: f64, vy: f64]
    pub fn serialize_state(&self) -> Vec<u8> {
        // Count non-fixed bodies (static bodies never move, skip them)
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

        // 4 bytes count + 44 bytes per body (4 + 5*8)
        let mut buf = Vec::with_capacity(4 + count * 44);
        buf.extend_from_slice(&(count as u32).to_le_bytes());

        for (&body_id, &handle) in &self.handle_map {
            let rb = match self.rigid_body_set.get(handle) {
                Some(rb) => rb,
                None => continue,
            };
            if rb.is_fixed() {
                continue;
            }

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

        buf
    }

    /// Ray cast into the 2D physics world.
    /// Returns the first hit: (body_id, hit_x, hit_y, normal_x, normal_y, toi).
    pub fn ray_cast(&self, ox: f32, oy: f32, dx: f32, dy: f32, max_dist: f32) -> Option<(u32, f32, f32, f32, f32, f32)> {
        let ray = Ray::new(point![ox, oy], vector![dx, dy]);
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
                        hit_point.x,
                        hit_point.y,
                        intersection.normal.x,
                        intersection.normal.y,
                        intersection.time_of_impact,
                    ));
                }
            }
        }
        None
    }

    /// Query all bodies whose colliders intersect an AABB rectangle.
    /// Returns a list of body_ids.
    pub fn query_rect(&self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Vec<u32> {
        let aabb = Aabb {
            mins: point![min_x, min_y],
            maxs: point![max_x, max_y],
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

    /// Get contact events from the narrow phase.
    /// Returns pairs of body IDs that are in contact.
    pub fn get_contacts(&self) -> Vec<(u32, u32)> {
        let mut contacts = Vec::new();
        for pair in self.narrow_phase.contact_pairs() {
            if !pair.has_any_active_contact {
                continue;
            }
            let rb1 = self.collider_set.get(pair.collider1).and_then(|c| c.parent());
            let rb2 = self.collider_set.get(pair.collider2).and_then(|c| c.parent());
            if let (Some(h1), Some(h2)) = (rb1, rb2) {
                if let (Some(&id1), Some(&id2)) = (
                    self.reverse_map.get(&h1),
                    self.reverse_map.get(&h2),
                ) {
                    contacts.push((id1, id2));
                }
            }
        }
        contacts
    }
}
