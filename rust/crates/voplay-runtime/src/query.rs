use std::collections::{BTreeMap, BTreeSet};

use crate::{
    sparse_store::{ComponentChange, ComponentChangeKind},
    world::{WorldEntity, WorldId},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryIndexConfig {
    pub max_components: usize,
    pub max_memberships: usize,
}

impl Default for QueryIndexConfig {
    fn default() -> Self {
        Self {
            max_components: 4096,
            max_memberships: 16_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryError {
    Closed,
    InvalidConfig,
    InvalidComponent,
    DuplicateComponent,
    RequiredExcludedOverlap,
    WrongWorld,
    ComponentCapacity,
    MembershipCapacity,
    JournalSequence,
    PresenceMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryPlan {
    world: WorldId,
    required: BTreeSet<u32>,
    excluded: BTreeSet<u32>,
}

impl QueryPlan {
    pub fn new(
        world: WorldId,
        required: impl IntoIterator<Item = u32>,
        excluded: impl IntoIterator<Item = u32>,
    ) -> Result<Self, QueryError> {
        if !world.is_valid() {
            return Err(QueryError::InvalidConfig);
        }
        let required = collect_components(required)?;
        let excluded = collect_components(excluded)?;
        if required.is_empty() {
            return Err(QueryError::InvalidComponent);
        }
        if required.iter().any(|item| excluded.contains(item)) {
            return Err(QueryError::RequiredExcludedOverlap);
        }
        Ok(Self {
            world,
            required,
            excluded,
        })
    }

    pub fn execute(&self, index: &ComponentPresenceIndex) -> Result<Vec<WorldEntity>, QueryError> {
        index.ensure_open()?;
        if index.world != self.world {
            return Err(QueryError::WrongWorld);
        }
        let Some(seed) = self
            .required
            .iter()
            .filter_map(|component| index.components.get(component))
            .min_by_key(|entities| entities.len())
        else {
            return Ok(Vec::new());
        };
        Ok(seed
            .iter()
            .copied()
            .filter(|entity| {
                self.required.iter().all(|component| {
                    index
                        .components
                        .get(component)
                        .is_some_and(|entities| entities.contains(entity))
                }) && self.excluded.iter().all(|component| {
                    index
                        .components
                        .get(component)
                        .is_none_or(|entities| !entities.contains(entity))
                })
            })
            .collect())
    }
}

pub struct ComponentPresenceIndex {
    world: WorldId,
    config: QueryIndexConfig,
    memberships: usize,
    components: BTreeMap<u32, BTreeSet<WorldEntity>>,
    component_revisions: BTreeMap<u32, u64>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentPresenceOwnerSnapshot {
    pub closed: bool,
    pub components: usize,
    pub memberships: usize,
    pub component_revisions: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentPresenceShutdownReport {
    pub released_components: usize,
    pub released_memberships: usize,
    pub released_component_revisions: usize,
}

impl ComponentPresenceIndex {
    pub fn new(world: WorldId, config: QueryIndexConfig) -> Result<Self, QueryError> {
        if !world.is_valid() || config.max_components == 0 || config.max_memberships == 0 {
            return Err(QueryError::InvalidConfig);
        }
        Ok(Self {
            world,
            config,
            memberships: 0,
            components: BTreeMap::new(),
            component_revisions: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> ComponentPresenceOwnerSnapshot {
        ComponentPresenceOwnerSnapshot {
            closed: self.closed,
            components: self.components.len(),
            memberships: self.memberships,
            component_revisions: self.component_revisions.len(),
        }
    }

    pub fn shutdown(&mut self) -> ComponentPresenceShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.memberships = 0;
        self.components.clear();
        self.component_revisions.clear();
        ComponentPresenceShutdownReport {
            released_components: snapshot.components,
            released_memberships: snapshot.memberships,
            released_component_revisions: snapshot.component_revisions,
        }
    }

    pub fn apply_component_journal(
        &mut self,
        component: u32,
        journal: &[ComponentChange],
    ) -> Result<(), QueryError> {
        self.ensure_open()?;
        if component == 0 {
            return Err(QueryError::InvalidComponent);
        }
        if journal.is_empty() {
            return Ok(());
        }
        if !self.components.contains_key(&component)
            && self.components.len() == self.config.max_components
        {
            return Err(QueryError::ComponentCapacity);
        }
        let mut revision = self
            .component_revisions
            .get(&component)
            .copied()
            .unwrap_or(0);
        let mut staged = self.components.get(&component).cloned().unwrap_or_default();
        let original_len = staged.len();
        for change in journal {
            if change.entity.world != self.world || !change.entity.entity.is_valid() {
                return Err(QueryError::WrongWorld);
            }
            if change.revision <= revision {
                return Err(QueryError::JournalSequence);
            }
            revision = change.revision;
            match change.kind {
                ComponentChangeKind::Added => {
                    if !staged.insert(change.entity) {
                        return Err(QueryError::PresenceMismatch);
                    }
                }
                ComponentChangeKind::Changed => {
                    if !staged.contains(&change.entity) {
                        return Err(QueryError::PresenceMismatch);
                    }
                }
                ComponentChangeKind::Removed => {
                    if !staged.remove(&change.entity) {
                        return Err(QueryError::PresenceMismatch);
                    }
                }
            }
        }
        let memberships = self
            .memberships
            .checked_sub(original_len)
            .and_then(|count| count.checked_add(staged.len()))
            .filter(|count| *count <= self.config.max_memberships)
            .ok_or(QueryError::MembershipCapacity)?;
        self.memberships = memberships;
        self.components.insert(component, staged);
        self.component_revisions.insert(component, revision);
        Ok(())
    }

    pub const fn memberships(&self) -> usize {
        self.memberships
    }

    fn ensure_open(&self) -> Result<(), QueryError> {
        if self.closed {
            Err(QueryError::Closed)
        } else {
            Ok(())
        }
    }
}

fn collect_components(
    components: impl IntoIterator<Item = u32>,
) -> Result<BTreeSet<u32>, QueryError> {
    let mut result = BTreeSet::new();
    for component in components {
        if component == 0 {
            return Err(QueryError::InvalidComponent);
        }
        if !result.insert(component) {
            return Err(QueryError::DuplicateComponent);
        }
    }
    Ok(result)
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

    fn world_id(engine: u32) -> WorldId {
        WorldId {
            engine: handle(engine),
            handle: handle(1),
        }
    }

    fn entity(world: WorldId, index: u32) -> WorldEntity {
        WorldEntity {
            world,
            entity: handle(index),
        }
    }

    fn change(revision: u64, entity: WorldEntity, kind: ComponentChangeKind) -> ComponentChange {
        ComponentChange {
            revision,
            entity,
            kind,
        }
    }

    #[test]
    fn plan_intersects_and_excludes_in_stable_entity_order() {
        let world = world_id(1);
        let mut index = ComponentPresenceIndex::new(world, QueryIndexConfig::default()).unwrap();
        let one = entity(world, 1);
        let two = entity(world, 2);
        let three = entity(world, 3);
        index
            .apply_component_journal(
                10,
                &[
                    change(1, three, ComponentChangeKind::Added),
                    change(2, one, ComponentChangeKind::Added),
                    change(3, two, ComponentChangeKind::Added),
                ],
            )
            .unwrap();
        index
            .apply_component_journal(
                20,
                &[
                    change(1, two, ComponentChangeKind::Added),
                    change(2, one, ComponentChangeKind::Added),
                ],
            )
            .unwrap();
        index
            .apply_component_journal(30, &[change(1, two, ComponentChangeKind::Added)])
            .unwrap();
        let plan = QueryPlan::new(world, [10, 20], [30]).unwrap();
        assert_eq!(plan.execute(&index), Ok(vec![one]));
    }

    #[test]
    fn malformed_journal_is_atomic_and_owner_qualified() {
        let world = world_id(1);
        let mut index = ComponentPresenceIndex::new(
            world,
            QueryIndexConfig {
                max_components: 2,
                max_memberships: 2,
            },
        )
        .unwrap();
        let live = entity(world, 1);
        index
            .apply_component_journal(10, &[change(1, live, ComponentChangeKind::Added)])
            .unwrap();
        assert_eq!(
            index.apply_component_journal(
                10,
                &[
                    change(2, live, ComponentChangeKind::Removed),
                    change(3, entity(world_id(2), 1), ComponentChangeKind::Added,),
                ],
            ),
            Err(QueryError::WrongWorld)
        );
        assert_eq!(
            QueryPlan::new(world, [10], []).unwrap().execute(&index),
            Ok(vec![live])
        );
        assert_eq!(
            index.apply_component_journal(10, &[change(1, live, ComponentChangeKind::Changed)]),
            Err(QueryError::JournalSequence)
        );
    }

    #[test]
    fn query_plan_rejects_ambiguous_component_contracts() {
        let world = world_id(1);
        assert_eq!(
            QueryPlan::new(world, [1, 1], []),
            Err(QueryError::DuplicateComponent)
        );
        assert_eq!(
            QueryPlan::new(world, [1], [1]),
            Err(QueryError::RequiredExcludedOverlap)
        );
        assert_eq!(
            QueryPlan::new(world, [], [1]),
            Err(QueryError::InvalidComponent)
        );
    }
}
