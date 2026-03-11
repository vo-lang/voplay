//! Generic runtime world registry.
//!
//! Multiple runtime subsystems use the same pattern: a global Mutex-guarded
//! HashMap of worlds keyed by u32 handle. This module extracts that
//! lifecycle boilerplate into a reusable generic.

use std::collections::HashMap;
use std::sync::Mutex;

/// A registry that manages worlds of type `W`, keyed by u32 handles.
pub struct WorldRegistry<W> {
    worlds: HashMap<u32, W>,
    next_id: u32,
}

impl<W> WorldRegistry<W> {
    pub fn new() -> Self {
        Self {
            worlds: HashMap::new(),
            next_id: 1,
        }
    }

    /// Insert a new world and return its handle.
    pub fn insert(&mut self, world: W) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.worlds.insert(id, world);
        id
    }

    /// Remove a world by handle.
    pub fn remove(&mut self, id: u32) {
        self.worlds.remove(&id);
    }

    /// Access a world mutably by handle. Panics if not found.
    pub fn get_mut(&mut self, id: u32) -> &mut W {
        self.worlds.get_mut(&id)
            .expect("voplay: world not found")
    }

    /// Access a world immutably by handle.
    pub fn get(&self, id: u32) -> Option<&W> {
        self.worlds.get(&id)
    }
}

/// Helper to initialize-or-get a registry inside a Mutex<Option<...>>,
/// then run a closure with mutable access to a specific world.
pub fn with_world_in<W, R>(
    mutex: &Mutex<Option<WorldRegistry<W>>>,
    world_id: u32,
    f: impl FnOnce(&mut W) -> R,
) -> R {
    let mut guard = mutex.lock().unwrap();
    let reg = guard.as_mut().expect("voplay: world registry not initialized");
    let world = reg.get_mut(world_id);
    f(world)
}

/// Helper to access a registry immutably inside a Mutex<Option<...>>,
/// then run a closure with immutable access to a specific world.
pub fn with_world_ref_in<W, R>(
    mutex: &Mutex<Option<WorldRegistry<W>>>,
    world_id: u32,
    f: impl FnOnce(&W) -> R,
) -> Option<R> {
    let guard = mutex.lock().unwrap();
    let reg = guard.as_ref()?;
    let world = reg.get(world_id)?;
    Some(f(world))
}

/// Shared body type enum used by both 2D and 3D physics.
/// Must match Vo's BodyType const values: Dynamic=0, Static=1, Kinematic=2.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysBodyType {
    Dynamic = 0,
    Static = 1,
    Kinematic = 2,
}

impl PhysBodyType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => PhysBodyType::Static,
            2 => PhysBodyType::Kinematic,
            _ => PhysBodyType::Dynamic,
        }
    }
}
