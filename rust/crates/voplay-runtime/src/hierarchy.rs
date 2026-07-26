use std::collections::{BTreeMap, BTreeSet};

use crate::world::{WorldEntity, WorldId};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Transform {
    pub translation: [i64; 3],
}

impl Transform {
    fn compose(self, local: Self) -> Option<Self> {
        Some(Self {
            translation: [
                self.translation[0].checked_add(local.translation[0])?,
                self.translation[1].checked_add(local.translation[1])?,
                self.translation[2].checked_add(local.translation[2])?,
            ],
        })
    }

    fn relative_to(self, parent: Self) -> Option<Self> {
        Some(Self {
            translation: [
                self.translation[0].checked_sub(parent.translation[0])?,
                self.translation[1].checked_sub(parent.translation[1])?,
                self.translation[2].checked_sub(parent.translation[2])?,
            ],
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchyConfig {
    pub max_nodes: usize,
    pub max_depth: usize,
}

impl Default for HierarchyConfig {
    fn default() -> Self {
        Self {
            max_nodes: 1_000_000,
            max_depth: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReparentMode {
    KeepLocal,
    KeepWorld,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteMode {
    Recursive,
    ReparentChildrenKeepWorld,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HierarchyError {
    Closed,
    InvalidConfig,
    WrongWorld,
    NodeCapacity,
    NodeAlreadyExists,
    NodeNotFound,
    Cycle,
    DepthCapacity,
    TransformOverflow,
}

#[derive(Clone, Debug)]
struct HierarchyNode {
    parent: Option<WorldEntity>,
    children: BTreeSet<WorldEntity>,
    local: Transform,
    world: Transform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchyInspectionNode {
    pub entity: WorldEntity,
    pub parent: Option<WorldEntity>,
    pub local: Transform,
    pub world: Transform,
    pub child_count: usize,
}

pub struct Hierarchy {
    world: WorldId,
    config: HierarchyConfig,
    nodes: BTreeMap<WorldEntity, HierarchyNode>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchyOwnerSnapshot {
    pub closed: bool,
    pub nodes: usize,
    pub parent_edges: usize,
    pub child_edges: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchyShutdownReport {
    pub released_nodes: usize,
    pub released_parent_edges: usize,
    pub released_child_edges: usize,
}

impl Hierarchy {
    pub fn new(world: WorldId, config: HierarchyConfig) -> Result<Self, HierarchyError> {
        if !world.is_valid() || config.max_nodes == 0 || config.max_depth == 0 {
            return Err(HierarchyError::InvalidConfig);
        }
        Ok(Self {
            world,
            config,
            nodes: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> HierarchyOwnerSnapshot {
        HierarchyOwnerSnapshot {
            closed: self.closed,
            nodes: self.nodes.len(),
            parent_edges: self
                .nodes
                .values()
                .filter(|node| node.parent.is_some())
                .count(),
            child_edges: self.nodes.values().map(|node| node.children.len()).sum(),
        }
    }

    pub fn shutdown(&mut self) -> HierarchyShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.nodes.clear();
        HierarchyShutdownReport {
            released_nodes: snapshot.nodes,
            released_parent_edges: snapshot.parent_edges,
            released_child_edges: snapshot.child_edges,
        }
    }

    pub fn insert(
        &mut self,
        entity: WorldEntity,
        parent: Option<WorldEntity>,
        local: Transform,
    ) -> Result<(), HierarchyError> {
        self.ensure_open()?;
        self.validate_owner(entity)?;
        if self.nodes.contains_key(&entity) {
            return Err(HierarchyError::NodeAlreadyExists);
        }
        if self.nodes.len() == self.config.max_nodes {
            return Err(HierarchyError::NodeCapacity);
        }
        let (parent_world, depth) = if let Some(parent) = parent {
            self.validate_owner(parent)?;
            let parent_node = self
                .nodes
                .get(&parent)
                .ok_or(HierarchyError::NodeNotFound)?;
            (parent_node.world, self.depth(parent)?.checked_add(1))
        } else {
            (Transform::default(), Some(1))
        };
        if depth
            .filter(|depth| *depth <= self.config.max_depth)
            .is_none()
        {
            return Err(HierarchyError::DepthCapacity);
        }
        let world = parent_world
            .compose(local)
            .ok_or(HierarchyError::TransformOverflow)?;
        self.nodes.insert(
            entity,
            HierarchyNode {
                parent,
                children: BTreeSet::new(),
                local,
                world,
            },
        );
        if let Some(parent) = parent {
            self.nodes.get_mut(&parent).unwrap().children.insert(entity);
        }
        Ok(())
    }

    pub fn reparent(
        &mut self,
        entity: WorldEntity,
        new_parent: Option<WorldEntity>,
        mode: ReparentMode,
    ) -> Result<(), HierarchyError> {
        self.ensure_open()?;
        self.validate_owner(entity)?;
        let node = self
            .nodes
            .get(&entity)
            .ok_or(HierarchyError::NodeNotFound)?;
        let old_parent = node.parent;
        if old_parent == new_parent {
            return Ok(());
        }
        let old_world = node.world;
        let old_parent_world = old_parent
            .map(|parent| self.nodes[&parent].world)
            .unwrap_or_default();
        let (new_parent_world, new_parent_depth) = if let Some(parent) = new_parent {
            self.validate_owner(parent)?;
            if !self.nodes.contains_key(&parent) {
                return Err(HierarchyError::NodeNotFound);
            }
            if parent == entity || self.is_descendant(parent, entity) {
                return Err(HierarchyError::Cycle);
            }
            (self.nodes[&parent].world, self.depth(parent)?)
        } else {
            (Transform::default(), 0)
        };
        let subtree_depth = self.subtree_depth(entity)?;
        if new_parent_depth
            .checked_add(subtree_depth)
            .filter(|depth| *depth <= self.config.max_depth)
            .is_none()
        {
            return Err(HierarchyError::DepthCapacity);
        }

        let new_local = match mode {
            ReparentMode::KeepLocal => node.local,
            ReparentMode::KeepWorld => old_world
                .relative_to(new_parent_world)
                .ok_or(HierarchyError::TransformOverflow)?,
        };
        let delta = match mode {
            ReparentMode::KeepLocal => new_parent_world
                .relative_to(old_parent_world)
                .ok_or(HierarchyError::TransformOverflow)?,
            ReparentMode::KeepWorld => Transform::default(),
        };
        let subtree = self.subtree(entity);
        let staged_worlds = subtree
            .iter()
            .map(|item| {
                self.nodes[item]
                    .world
                    .compose(delta)
                    .map(|world| (*item, world))
                    .ok_or(HierarchyError::TransformOverflow)
            })
            .collect::<Result<Vec<_>, _>>()?;

        if let Some(parent) = old_parent {
            self.nodes
                .get_mut(&parent)
                .unwrap()
                .children
                .remove(&entity);
        }
        if let Some(parent) = new_parent {
            self.nodes.get_mut(&parent).unwrap().children.insert(entity);
        }
        let node = self.nodes.get_mut(&entity).unwrap();
        node.parent = new_parent;
        node.local = new_local;
        for (item, world) in staged_worlds {
            self.nodes.get_mut(&item).unwrap().world = world;
        }
        Ok(())
    }

    pub fn delete(
        &mut self,
        entity: WorldEntity,
        mode: DeleteMode,
    ) -> Result<Vec<WorldEntity>, HierarchyError> {
        self.ensure_open()?;
        self.validate_owner(entity)?;
        let node = self
            .nodes
            .get(&entity)
            .ok_or(HierarchyError::NodeNotFound)?;
        let parent = node.parent;
        let children = node.children.iter().copied().collect::<Vec<_>>();
        if mode == DeleteMode::ReparentChildrenKeepWorld {
            let parent_world = parent
                .map(|parent| self.nodes[&parent].world)
                .unwrap_or_default();
            let staged_locals = children
                .iter()
                .map(|child| {
                    self.nodes[child]
                        .world
                        .relative_to(parent_world)
                        .map(|local| (*child, local))
                        .ok_or(HierarchyError::TransformOverflow)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(parent) = parent {
                let parent_node = self.nodes.get_mut(&parent).unwrap();
                parent_node.children.remove(&entity);
                parent_node.children.extend(children.iter().copied());
            }
            for (child, local) in staged_locals {
                let child_node = self.nodes.get_mut(&child).unwrap();
                child_node.parent = parent;
                child_node.local = local;
            }
            self.nodes.remove(&entity);
            return Ok(vec![entity]);
        }

        let removed = self.subtree(entity);
        if let Some(parent) = parent {
            self.nodes
                .get_mut(&parent)
                .unwrap()
                .children
                .remove(&entity);
        }
        for item in &removed {
            self.nodes.remove(item);
        }
        Ok(removed)
    }

    pub fn parent(&self, entity: WorldEntity) -> Option<Option<WorldEntity>> {
        self.nodes.get(&entity).map(|node| node.parent)
    }

    pub const fn world(&self) -> WorldId {
        self.world
    }

    pub fn inspection_revision(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        hash_hierarchy_u32(&mut hash, self.world.handle.index);
        hash_hierarchy_u32(&mut hash, self.world.handle.generation);
        for (entity, node) in &self.nodes {
            hash_hierarchy_u32(&mut hash, entity.entity.index);
            hash_hierarchy_u32(&mut hash, entity.entity.generation);
            if let Some(parent) = node.parent {
                hash_hierarchy_u32(&mut hash, 1);
                hash_hierarchy_u32(&mut hash, parent.entity.index);
                hash_hierarchy_u32(&mut hash, parent.entity.generation);
            } else {
                hash_hierarchy_u32(&mut hash, 0);
            }
            for value in node.local.translation {
                hash_hierarchy_u64(&mut hash, value as u64);
            }
            for value in node.world.translation {
                hash_hierarchy_u64(&mut hash, value as u64);
            }
            hash_hierarchy_u64(&mut hash, node.children.len() as u64);
        }
        hash
    }

    pub fn inspection_nodes(&self) -> Vec<HierarchyInspectionNode> {
        self.nodes
            .iter()
            .map(|(entity, node)| HierarchyInspectionNode {
                entity: *entity,
                parent: node.parent,
                local: node.local,
                world: node.world,
                child_count: node.children.len(),
            })
            .collect()
    }

    pub fn local_transform(&self, entity: WorldEntity) -> Option<Transform> {
        self.nodes.get(&entity).map(|node| node.local)
    }

    pub fn world_transform(&self, entity: WorldEntity) -> Option<Transform> {
        self.nodes.get(&entity).map(|node| node.world)
    }

    fn validate_owner(&self, entity: WorldEntity) -> Result<(), HierarchyError> {
        self.ensure_open()?;
        if entity.world != self.world || !entity.entity.is_valid() {
            return Err(HierarchyError::WrongWorld);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), HierarchyError> {
        if self.closed {
            Err(HierarchyError::Closed)
        } else {
            Ok(())
        }
    }

    fn depth(&self, mut entity: WorldEntity) -> Result<usize, HierarchyError> {
        let mut depth = 0_usize;
        loop {
            depth = depth.checked_add(1).ok_or(HierarchyError::DepthCapacity)?;
            let node = self
                .nodes
                .get(&entity)
                .ok_or(HierarchyError::NodeNotFound)?;
            let Some(parent) = node.parent else {
                return Ok(depth);
            };
            entity = parent;
        }
    }

    fn subtree_depth(&self, root: WorldEntity) -> Result<usize, HierarchyError> {
        let mut deepest = 0;
        let mut pending = vec![(root, 1_usize)];
        while let Some((entity, depth)) = pending.pop() {
            deepest = deepest.max(depth);
            let node = self
                .nodes
                .get(&entity)
                .ok_or(HierarchyError::NodeNotFound)?;
            pending.extend(node.children.iter().map(|child| (*child, depth + 1)));
        }
        Ok(deepest)
    }

    fn is_descendant(&self, entity: WorldEntity, ancestor: WorldEntity) -> bool {
        let mut current = Some(entity);
        while let Some(item) = current {
            if item == ancestor {
                return true;
            }
            current = self.nodes.get(&item).and_then(|node| node.parent);
        }
        false
    }

    fn subtree(&self, root: WorldEntity) -> Vec<WorldEntity> {
        let mut result = Vec::new();
        let mut pending = vec![root];
        while let Some(entity) = pending.pop() {
            result.push(entity);
            pending.extend(self.nodes[&entity].children.iter().rev().copied());
        }
        result
    }
}

fn hash_hierarchy_u32(hash: &mut u64, value: u32) {
    hash_hierarchy_bytes(hash, &value.to_le_bytes());
}

fn hash_hierarchy_u64(hash: &mut u64, value: u64) {
    hash_hierarchy_bytes(hash, &value.to_le_bytes());
}

fn hash_hierarchy_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = (*hash).wrapping_mul(0x100000001b3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn world_id(engine: u32, world: u32) -> WorldId {
        WorldId {
            engine: handle(engine),
            handle: handle(world),
        }
    }

    fn entity(world: WorldId, index: u32) -> WorldEntity {
        WorldEntity {
            world,
            entity: handle(index),
        }
    }

    fn transform(x: i64) -> Transform {
        Transform {
            translation: [x, 0, 0],
        }
    }

    #[test]
    fn reparent_modes_preserve_the_declared_transform_domain() {
        let world = world_id(1, 1);
        let mut hierarchy = Hierarchy::new(world, HierarchyConfig::default()).unwrap();
        let left = entity(world, 1);
        let right = entity(world, 2);
        let child = entity(world, 3);
        hierarchy.insert(left, None, transform(10)).unwrap();
        hierarchy.insert(right, None, transform(100)).unwrap();
        hierarchy.insert(child, Some(left), transform(2)).unwrap();
        hierarchy
            .reparent(child, Some(right), ReparentMode::KeepWorld)
            .unwrap();
        assert_eq!(hierarchy.world_transform(child), Some(transform(12)));
        assert_eq!(hierarchy.local_transform(child), Some(transform(-88)));
        hierarchy
            .reparent(child, Some(left), ReparentMode::KeepLocal)
            .unwrap();
        assert_eq!(hierarchy.local_transform(child), Some(transform(-88)));
        assert_eq!(hierarchy.world_transform(child), Some(transform(-78)));
    }

    #[test]
    fn cycle_wrong_world_and_depth_fail_before_mutation() {
        let world = world_id(1, 1);
        let mut hierarchy = Hierarchy::new(
            world,
            HierarchyConfig {
                max_nodes: 4,
                max_depth: 2,
            },
        )
        .unwrap();
        let root = entity(world, 1);
        let child = entity(world, 2);
        hierarchy.insert(root, None, transform(1)).unwrap();
        hierarchy.insert(child, Some(root), transform(2)).unwrap();
        assert_eq!(
            hierarchy.reparent(root, Some(child), ReparentMode::KeepWorld),
            Err(HierarchyError::Cycle)
        );
        assert_eq!(hierarchy.parent(root), Some(None));
        assert_eq!(
            hierarchy.insert(entity(world_id(2, 1), 3), None, Transform::default()),
            Err(HierarchyError::WrongWorld)
        );
        assert_eq!(
            hierarchy.insert(entity(world, 3), Some(child), Transform::default()),
            Err(HierarchyError::DepthCapacity)
        );
    }

    #[test]
    fn delete_can_reparent_children_keep_world_or_remove_subtree() {
        let world = world_id(1, 1);
        let mut hierarchy = Hierarchy::new(world, HierarchyConfig::default()).unwrap();
        let root = entity(world, 1);
        let branch = entity(world, 2);
        let leaf = entity(world, 3);
        hierarchy.insert(root, None, transform(10)).unwrap();
        hierarchy.insert(branch, Some(root), transform(2)).unwrap();
        hierarchy.insert(leaf, Some(branch), transform(3)).unwrap();
        assert_eq!(
            hierarchy
                .delete(branch, DeleteMode::ReparentChildrenKeepWorld)
                .unwrap(),
            vec![branch]
        );
        assert_eq!(hierarchy.parent(leaf), Some(Some(root)));
        assert_eq!(hierarchy.local_transform(leaf), Some(transform(5)));
        assert_eq!(hierarchy.world_transform(leaf), Some(transform(15)));
        assert_eq!(
            hierarchy.delete(root, DeleteMode::Recursive).unwrap(),
            vec![root, leaf]
        );
        assert!(hierarchy.world_transform(leaf).is_none());
    }
}
