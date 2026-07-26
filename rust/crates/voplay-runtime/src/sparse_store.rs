use crate::world::{WorldEntity, WorldId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentChangeKind {
    Added,
    Changed,
    Removed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentChange {
    pub revision: u64,
    pub entity: WorldEntity,
    pub kind: ComponentChangeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SparseStoreError {
    Closed,
    InvalidConfig,
    InvalidEntity,
    Capacity,
    Missing,
    RevisionExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SparseStoreConfig {
    pub max_items: usize,
    pub max_entity_slots: usize,
}

#[derive(Clone, Copy, Debug)]
struct SparseEntry {
    generation: u32,
    dense_index: usize,
}

pub struct SparseStore<T> {
    world: WorldId,
    max_items: usize,
    max_entity_slots: usize,
    revision: u64,
    sparse: Vec<Option<SparseEntry>>,
    dense_entities: Vec<WorldEntity>,
    dense_values: Vec<T>,
    journal: Vec<ComponentChange>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SparseStoreOwnerSnapshot {
    pub closed: bool,
    pub live_items: usize,
    pub sparse_slots: usize,
    pub pending_changes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SparseStoreShutdownReport {
    pub released_items: usize,
    pub released_sparse_slots: usize,
    pub released_changes: usize,
}

impl<T: PartialEq> SparseStore<T> {
    pub fn new(world: WorldId, config: SparseStoreConfig) -> Result<Self, SparseStoreError> {
        if !world.is_valid() || config.max_items == 0 || config.max_entity_slots == 0 {
            return Err(SparseStoreError::InvalidConfig);
        }
        Ok(Self {
            world,
            max_items: config.max_items,
            max_entity_slots: config.max_entity_slots,
            revision: 0,
            sparse: Vec::new(),
            dense_entities: Vec::new(),
            dense_values: Vec::new(),
            journal: Vec::new(),
            closed: false,
        })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.dense_entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dense_entities.is_empty()
    }

    pub fn owner_snapshot(&self) -> SparseStoreOwnerSnapshot {
        SparseStoreOwnerSnapshot {
            closed: self.closed,
            live_items: self.dense_values.len(),
            sparse_slots: self.sparse.len(),
            pending_changes: self.journal.len(),
        }
    }

    pub fn shutdown(&mut self) -> SparseStoreShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.sparse.clear();
        self.dense_entities.clear();
        self.dense_values.clear();
        self.journal.clear();
        SparseStoreShutdownReport {
            released_items: snapshot.live_items,
            released_sparse_slots: snapshot.sparse_slots,
            released_changes: snapshot.pending_changes,
        }
    }

    pub fn get(&self, entity: WorldEntity) -> Option<&T> {
        let dense_index = self.dense_index(entity)?;
        self.dense_values.get(dense_index)
    }

    pub fn insert(&mut self, entity: WorldEntity, value: T) -> Result<bool, SparseStoreError> {
        self.ensure_open()?;
        self.validate_entity(entity)?;
        if let Some(dense_index) = self.dense_index(entity) {
            if self.dense_values[dense_index] == value {
                return Ok(false);
            }
            let revision = self.next_revision()?;
            self.dense_values[dense_index] = value;
            self.revision = revision;
            self.journal.push(ComponentChange {
                revision,
                entity,
                kind: ComponentChangeKind::Changed,
            });
            return Ok(true);
        }
        let sparse_index = entity.entity.index as usize;
        if sparse_index >= self.max_entity_slots
            || self
                .sparse
                .get(sparse_index)
                .and_then(Option::as_ref)
                .is_some()
        {
            return Err(SparseStoreError::InvalidEntity);
        }
        if self.dense_entities.len() == self.max_items {
            return Err(SparseStoreError::Capacity);
        }
        let revision = self.next_revision()?;
        if self.sparse.len() <= sparse_index {
            self.sparse.resize(sparse_index + 1, None);
        }
        let dense_index = self.dense_entities.len();
        self.dense_entities.push(entity);
        self.dense_values.push(value);
        self.sparse[sparse_index] = Some(SparseEntry {
            generation: entity.entity.generation,
            dense_index,
        });
        self.revision = revision;
        self.journal.push(ComponentChange {
            revision,
            entity,
            kind: ComponentChangeKind::Added,
        });
        Ok(true)
    }

    pub fn remove(&mut self, entity: WorldEntity) -> Result<T, SparseStoreError> {
        self.ensure_open()?;
        self.validate_entity(entity)?;
        let dense_index = self.dense_index(entity).ok_or(SparseStoreError::Missing)?;
        let revision = self.next_revision()?;
        let last_index = self.dense_entities.len() - 1;
        let removed_value = self.dense_values.swap_remove(dense_index);
        self.dense_entities.swap_remove(dense_index);
        self.sparse[entity.entity.index as usize] = None;
        if dense_index != last_index {
            let moved = self.dense_entities[dense_index];
            self.sparse[moved.entity.index as usize]
                .as_mut()
                .expect("dense entity must have sparse entry")
                .dense_index = dense_index;
        }
        self.revision = revision;
        self.journal.push(ComponentChange {
            revision,
            entity,
            kind: ComponentChangeKind::Removed,
        });
        Ok(removed_value)
    }

    pub fn stable_entities(&self) -> Vec<WorldEntity> {
        let mut entities = self.dense_entities.clone();
        entities.sort_by_key(|entity| (entity.entity.index, entity.entity.generation));
        entities
    }

    pub fn drain_journal(&mut self) -> Vec<ComponentChange> {
        std::mem::take(&mut self.journal)
    }

    fn validate_entity(&self, entity: WorldEntity) -> Result<(), SparseStoreError> {
        self.ensure_open()?;
        if entity.world != self.world || !entity.entity.is_valid() {
            return Err(SparseStoreError::InvalidEntity);
        }
        Ok(())
    }

    fn dense_index(&self, entity: WorldEntity) -> Option<usize> {
        if entity.world != self.world || !entity.entity.is_valid() {
            return None;
        }
        let entry = self.sparse.get(entity.entity.index as usize)?.as_ref()?;
        if entry.generation != entity.entity.generation {
            return None;
        }
        Some(entry.dense_index)
    }

    fn next_revision(&self) -> Result<u64, SparseStoreError> {
        self.ensure_open()?;
        self.revision
            .checked_add(1)
            .ok_or(SparseStoreError::RevisionExhausted)
    }

    fn ensure_open(&self) -> Result<(), SparseStoreError> {
        if self.closed {
            Err(SparseStoreError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;

    fn owned(world: u32, entity: u32, generation: u32) -> WorldEntity {
        owned_in(1, world, entity, generation)
    }

    fn owned_in(engine: u32, world: u32, entity: u32, generation: u32) -> WorldEntity {
        WorldEntity {
            world: WorldId {
                engine: Handle {
                    index: engine,
                    generation: 1,
                },
                handle: Handle {
                    index: world,
                    generation: 1,
                },
            },
            entity: Handle {
                index: entity,
                generation,
            },
        }
    }

    fn config(max_items: usize) -> SparseStoreConfig {
        SparseStoreConfig {
            max_items,
            max_entity_slots: 16,
        }
    }

    #[test]
    fn add_change_remove_are_o1_indexed_and_journal_actual_changes() {
        let world = owned(1, 0, 1).world;
        let mut store = SparseStore::new(world, config(3)).unwrap();
        let alpha = owned(1, 7, 1);
        let beta = owned(1, 2, 1);
        assert_eq!(store.insert(alpha, 10), Ok(true));
        assert_eq!(store.insert(beta, 20), Ok(true));
        assert_eq!(store.insert(alpha, 10), Ok(false));
        assert_eq!(store.revision(), 2);
        assert_eq!(store.insert(alpha, 11), Ok(true));
        assert_eq!(store.remove(beta), Ok(20));
        assert_eq!(store.get(alpha), Some(&11));
        assert_eq!(store.stable_entities(), vec![alpha]);
        assert_eq!(
            store
                .drain_journal()
                .into_iter()
                .map(|change| change.kind)
                .collect::<Vec<_>>(),
            vec![
                ComponentChangeKind::Added,
                ComponentChangeKind::Added,
                ComponentChangeKind::Changed,
                ComponentChangeKind::Removed,
            ]
        );
    }

    #[test]
    fn world_and_generation_are_part_of_sparse_identity() {
        let mut store = SparseStore::new(owned(1, 0, 1).world, config(2)).unwrap();
        let live = owned(1, 0, 2);
        store.insert(live, 7).unwrap();
        assert_eq!(store.get(owned(1, 0, 1)), None);
        assert_eq!(store.get(owned(2, 0, 2)), None);
        assert_eq!(store.get(owned_in(2, 1, 0, 2)), None);
        assert_eq!(
            store.insert(owned(2, 0, 2), 8),
            Err(SparseStoreError::InvalidEntity)
        );
        assert_eq!(
            store.insert(owned_in(2, 1, 0, 2), 8),
            Err(SparseStoreError::InvalidEntity)
        );
        assert_eq!(
            store.insert(owned(1, 0, 3), 8),
            Err(SparseStoreError::InvalidEntity)
        );
        assert_eq!(
            store.insert(owned(1, 99, 1), 8),
            Err(SparseStoreError::InvalidEntity)
        );
    }

    #[test]
    fn swap_remove_repairs_moved_dense_index_and_stable_order() {
        let mut store = SparseStore::new(owned(1, 0, 1).world, config(4)).unwrap();
        let first = owned(1, 8, 1);
        let middle = owned(1, 3, 1);
        let last = owned(1, 5, 1);
        store.insert(first, 8).unwrap();
        store.insert(middle, 3).unwrap();
        store.insert(last, 5).unwrap();
        store.remove(middle).unwrap();
        assert_eq!(store.get(last), Some(&5));
        assert_eq!(store.stable_entities(), vec![last, first]);
    }
}
