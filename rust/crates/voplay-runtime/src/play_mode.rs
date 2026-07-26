use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::AssetScopeId,
    control::{ControlKind, StableControlRef},
    input::InputScopeId,
    world::{World, WorldCommand, WorldConfig, WorldError, WorldId},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlayModeId {
    pub editor_engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayModeConfig {
    pub max_sessions: usize,
    pub max_entities_per_session: usize,
    pub max_clone_bytes_per_session: usize,
    pub max_writeback_fields: usize,
}

impl Default for PlayModeConfig {
    fn default() -> Self {
        Self {
            max_sessions: 16,
            max_entities_per_session: 1_000_000,
            max_clone_bytes_per_session: 256 * 1024 * 1024,
            max_writeback_fields: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AuthoringField {
    pub component: u32,
    pub offset: usize,
    pub width: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayModePolicy {
    pub writeback_fields: Vec<AuthoringField>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlayModeResources {
    pub world: WorldId,
    pub input_scope: InputScopeId,
    pub render_target: StableControlRef,
    pub asset_scope: AssetScopeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayModeExit {
    Discard,
    ApplyAuthoring { expected_authoring_revision: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayModeStartReport {
    pub id: PlayModeId,
    pub authoring_world: WorldId,
    pub authoring_revision: u64,
    pub resources: PlayModeResources,
    pub cloned_entities: usize,
    pub cloned_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayModeExitReport {
    pub id: PlayModeId,
    pub resources: PlayModeResources,
    pub changed_components: usize,
    pub authoring_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayModeSessionOwnerSnapshot {
    pub id: PlayModeId,
    pub authoring_world: WorldId,
    pub authoring_revision: u64,
    pub resources: PlayModeResources,
    pub play_world_revision: u64,
    pub play_entities: usize,
    pub cloned_entities: usize,
    pub cloned_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayModeOwnerSnapshot {
    pub editor_engine: EngineId,
    pub closed: bool,
    pub live_sessions: usize,
    pub sessions: Vec<PlayModeSessionOwnerSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayModeShutdownReport {
    pub before: PlayModeOwnerSnapshot,
    pub released_sessions: Vec<PlayModeExitReport>,
    pub after: PlayModeOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayModeError {
    InvalidConfig,
    WrongEditorEngine,
    PlayEngineMustBeIsolated,
    DuplicatePlayEngine,
    DuplicatePlayWorld,
    DuplicateInputScope,
    DuplicateRenderTarget,
    DuplicateAssetScope,
    InvalidResources,
    SessionCapacity,
    EntityCapacity,
    CloneByteCapacity,
    WritebackFieldCapacity,
    InvalidWritebackField,
    OverlappingWritebackField,
    InvalidSession,
    StaleSession,
    WrongAuthoringWorld,
    RevisionConflict,
    MissingStableObject,
    ComponentMissing,
    ComponentShape,
    GenerationExhausted,
    Closed,
    World(WorldError),
}

impl From<WorldError> for PlayModeError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

struct PlayModeSession {
    authoring_world: WorldId,
    authoring_revision: u64,
    resources: PlayModeResources,
    play_world: World,
    baseline: BTreeMap<u64, BTreeMap<u32, Vec<u8>>>,
    policy: PlayModePolicy,
    cloned_entities: usize,
    cloned_bytes: usize,
}

struct PlayModeSlot {
    generation: u32,
    session: Option<PlayModeSession>,
}

pub struct PlayModeManager {
    editor_engine: EngineId,
    config: PlayModeConfig,
    slots: Vec<PlayModeSlot>,
    free: Vec<u32>,
    live: usize,
    closed: bool,
}

impl PlayModeManager {
    pub fn new(editor_engine: EngineId, config: PlayModeConfig) -> Result<Self, PlayModeError> {
        if !editor_engine.is_valid() {
            return Err(PlayModeError::WrongEditorEngine);
        }
        if config.max_sessions == 0
            || config.max_sessions > u32::MAX as usize
            || config.max_entities_per_session == 0
            || config.max_clone_bytes_per_session == 0
            || config.max_writeback_fields == 0
        {
            return Err(PlayModeError::InvalidConfig);
        }
        Ok(Self {
            editor_engine,
            config,
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            closed: false,
        })
    }

    pub const fn live_sessions(&self) -> usize {
        self.live
    }

    pub fn owner_snapshot(&self) -> PlayModeOwnerSnapshot {
        let sessions = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let session = slot.session.as_ref()?;
                Some(PlayModeSessionOwnerSnapshot {
                    id: PlayModeId {
                        editor_engine: self.editor_engine,
                        handle: Handle {
                            index: index as u32,
                            generation: slot.generation,
                        },
                    },
                    authoring_world: session.authoring_world,
                    authoring_revision: session.authoring_revision,
                    resources: session.resources,
                    play_world_revision: session.play_world.revision(),
                    play_entities: session.play_world.live_entities(),
                    cloned_entities: session.cloned_entities,
                    cloned_bytes: session.cloned_bytes,
                })
            })
            .collect();
        PlayModeOwnerSnapshot {
            editor_engine: self.editor_engine,
            closed: self.closed,
            live_sessions: self.live,
            sessions,
        }
    }

    pub fn start(
        &mut self,
        authoring: &World,
        resources: PlayModeResources,
        play_world_config: WorldConfig,
        policy: PlayModePolicy,
    ) -> Result<PlayModeStartReport, PlayModeError> {
        self.ensure_open()?;
        if authoring.id().engine != self.editor_engine {
            return Err(PlayModeError::WrongEditorEngine);
        }
        if resources.world.engine == self.editor_engine || !resources.world.is_valid() {
            return Err(PlayModeError::PlayEngineMustBeIsolated);
        }
        validate_resources(resources)?;
        if self.live == self.config.max_sessions {
            return Err(PlayModeError::SessionCapacity);
        }
        if self
            .slots
            .iter()
            .filter_map(|slot| slot.session.as_ref())
            .any(|session| session.play_world.id().engine == resources.world.engine)
        {
            return Err(PlayModeError::DuplicatePlayEngine);
        }
        if self
            .slots
            .iter()
            .filter_map(|slot| slot.session.as_ref())
            .any(|session| session.resources.world == resources.world)
        {
            return Err(PlayModeError::DuplicatePlayWorld);
        }
        if self
            .slots
            .iter()
            .filter_map(|slot| slot.session.as_ref())
            .any(|session| session.resources.input_scope == resources.input_scope)
        {
            return Err(PlayModeError::DuplicateInputScope);
        }
        if self
            .slots
            .iter()
            .filter_map(|slot| slot.session.as_ref())
            .any(|session| session.resources.render_target == resources.render_target)
        {
            return Err(PlayModeError::DuplicateRenderTarget);
        }
        if self
            .slots
            .iter()
            .filter_map(|slot| slot.session.as_ref())
            .any(|session| session.resources.asset_scope == resources.asset_scope)
        {
            return Err(PlayModeError::DuplicateAssetScope);
        }
        validate_policy(&policy, &self.config)?;
        let snapshot = authoring.snapshot()?;
        if snapshot
            .slots
            .iter()
            .filter(|slot| slot.stable_key.is_some())
            .count()
            > self.config.max_entities_per_session
        {
            return Err(PlayModeError::EntityCapacity);
        }
        let mut baseline = BTreeMap::new();
        let mut commands = Vec::new();
        let mut cloned_bytes = 0_usize;
        for slot in &snapshot.slots {
            let Some(stable_key) = slot.stable_key else {
                continue;
            };
            let component_bytes = slot
                .components
                .iter()
                .try_fold(0_usize, |bytes, (_, value)| {
                    bytes.checked_add(8)?.checked_add(value.len())
                });
            cloned_bytes = component_bytes
                .and_then(|bytes| cloned_bytes.checked_add(16)?.checked_add(bytes))
                .filter(|bytes| *bytes <= self.config.max_clone_bytes_per_session)
                .ok_or(PlayModeError::CloneByteCapacity)?;
            baseline.insert(stable_key, slot.components.clone());
            commands.push(WorldCommand::Spawn {
                stable_key,
                components: slot.components.clone(),
            });
        }
        let cloned_entities = commands.len();
        let mut isolated_world = World::new(resources.world, play_world_config)?;
        if !commands.is_empty() {
            isolated_world.apply_stage(commands)?;
        }
        let (index, generation) = self.allocate_slot()?;
        let id = PlayModeId {
            editor_engine: self.editor_engine,
            handle: Handle { index, generation },
        };
        self.slots[index as usize].session = Some(PlayModeSession {
            authoring_world: authoring.id(),
            authoring_revision: authoring.revision(),
            resources,
            play_world: isolated_world,
            baseline,
            policy,
            cloned_entities,
            cloned_bytes,
        });
        self.live += 1;
        Ok(PlayModeStartReport {
            id,
            authoring_world: authoring.id(),
            authoring_revision: authoring.revision(),
            resources,
            cloned_entities,
            cloned_bytes,
        })
    }

    pub fn play_world(&self, id: PlayModeId) -> Result<&World, PlayModeError> {
        Ok(&self.session(id)?.play_world)
    }

    pub fn play_world_mut(&mut self, id: PlayModeId) -> Result<&mut World, PlayModeError> {
        Ok(&mut self.session_mut(id)?.play_world)
    }

    pub fn exit(
        &mut self,
        id: PlayModeId,
        authoring: &mut World,
        exit: PlayModeExit,
    ) -> Result<PlayModeExitReport, PlayModeError> {
        if exit == PlayModeExit::Discard {
            return self.discard(id);
        }
        self.session(id)?;
        self.slots[id.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(PlayModeError::GenerationExhausted)?;
        let (resources, commands) = {
            let session = self.session(id)?;
            if authoring.id() != session.authoring_world {
                return Err(PlayModeError::WrongAuthoringWorld);
            }
            let commands = match exit {
                PlayModeExit::ApplyAuthoring {
                    expected_authoring_revision,
                } => {
                    if expected_authoring_revision != authoring.revision()
                        || authoring.revision() != session.authoring_revision
                    {
                        return Err(PlayModeError::RevisionConflict);
                    }
                    build_writeback_commands(session, authoring)?
                }
                PlayModeExit::Discard => unreachable!(),
            };
            (session.resources, commands)
        };
        let (changed_components, authoring_revision) = match exit {
            PlayModeExit::ApplyAuthoring { .. } if commands.is_empty() => {
                (0, Some(authoring.revision()))
            }
            PlayModeExit::ApplyAuthoring { .. } => {
                let result = authoring.apply_stage(commands)?;
                (result.change_count, Some(result.revision))
            }
            PlayModeExit::Discard => unreachable!(),
        };
        self.release_slot(id)?;
        Ok(PlayModeExitReport {
            id,
            resources,
            changed_components,
            authoring_revision,
        })
    }

    pub fn discard(&mut self, id: PlayModeId) -> Result<PlayModeExitReport, PlayModeError> {
        let resources = self.session(id)?.resources;
        self.release_slot(id)?;
        Ok(PlayModeExitReport {
            id,
            resources,
            changed_components: 0,
            authoring_revision: None,
        })
    }

    pub fn session_resources(&self, id: PlayModeId) -> Result<PlayModeResources, PlayModeError> {
        Ok(self.session(id)?.resources)
    }

    pub fn live_session_ids(&self) -> impl Iterator<Item = PlayModeId> + '_ {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.session.as_ref().map(|_| PlayModeId {
                editor_engine: self.editor_engine,
                handle: Handle {
                    index: index as u32,
                    generation: slot.generation,
                },
            })
        })
    }

    pub fn discard_all(&mut self) -> Result<Vec<PlayModeExitReport>, PlayModeError> {
        self.ensure_open()?;
        let ids = self.live_session_ids().collect::<Vec<_>>();
        let mut reports = Vec::with_capacity(ids.len());
        for id in &ids {
            let slot = &self.slots[id.handle.index as usize];
            slot.generation
                .checked_add(1)
                .ok_or(PlayModeError::GenerationExhausted)?;
            let session = slot.session.as_ref().ok_or(PlayModeError::StaleSession)?;
            reports.push(PlayModeExitReport {
                id: *id,
                resources: session.resources,
                changed_components: 0,
                authoring_revision: None,
            });
        }
        for id in ids {
            self.release_slot(id)?;
        }
        Ok(reports)
    }

    pub fn shutdown(&mut self) -> Result<PlayModeShutdownReport, PlayModeError> {
        let before = self.owner_snapshot();
        if self.closed {
            return Ok(PlayModeShutdownReport {
                before: before.clone(),
                released_sessions: Vec::new(),
                after: before,
            });
        }
        for slot in self.slots.iter().filter(|slot| slot.session.is_some()) {
            slot.generation
                .checked_add(1)
                .ok_or(PlayModeError::GenerationExhausted)?;
        }
        let released_sessions = self
            .live_session_ids()
            .map(|id| {
                let session = self.slots[id.handle.index as usize]
                    .session
                    .as_ref()
                    .expect("live session identity was derived from the same slot");
                PlayModeExitReport {
                    id,
                    resources: session.resources,
                    changed_components: 0,
                    authoring_revision: None,
                }
            })
            .collect::<Vec<_>>();
        self.free.clear();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.session.take().is_some() {
                slot.generation += 1;
            }
            self.free.push(index as u32);
        }
        self.live = 0;
        self.closed = true;
        Ok(PlayModeShutdownReport {
            before,
            released_sessions,
            after: self.owner_snapshot(),
        })
    }

    fn session(&self, id: PlayModeId) -> Result<&PlayModeSession, PlayModeError> {
        self.ensure_open()?;
        if id.editor_engine != self.editor_engine || !id.handle.is_valid() {
            return Err(PlayModeError::InvalidSession);
        }
        let slot = self
            .slots
            .get(id.handle.index as usize)
            .ok_or(PlayModeError::InvalidSession)?;
        if slot.generation != id.handle.generation || slot.session.is_none() {
            return Err(PlayModeError::StaleSession);
        }
        slot.session.as_ref().ok_or(PlayModeError::StaleSession)
    }

    fn session_mut(&mut self, id: PlayModeId) -> Result<&mut PlayModeSession, PlayModeError> {
        self.ensure_open()?;
        if id.editor_engine != self.editor_engine || !id.handle.is_valid() {
            return Err(PlayModeError::InvalidSession);
        }
        let slot = self
            .slots
            .get_mut(id.handle.index as usize)
            .ok_or(PlayModeError::InvalidSession)?;
        if slot.generation != id.handle.generation || slot.session.is_none() {
            return Err(PlayModeError::StaleSession);
        }
        slot.session.as_mut().ok_or(PlayModeError::StaleSession)
    }

    fn allocate_slot(&mut self) -> Result<(u32, u32), PlayModeError> {
        self.ensure_open()?;
        if let Some(index) = self.free.pop() {
            return Ok((index, self.slots[index as usize].generation));
        }
        let index = u32::try_from(self.slots.len()).map_err(|_| PlayModeError::SessionCapacity)?;
        self.slots.push(PlayModeSlot {
            generation: 1,
            session: None,
        });
        Ok((index, 1))
    }

    fn release_slot(&mut self, id: PlayModeId) -> Result<(), PlayModeError> {
        self.session(id)?;
        let slot = &mut self.slots[id.handle.index as usize];
        let generation = slot
            .generation
            .checked_add(1)
            .ok_or(PlayModeError::GenerationExhausted)?;
        slot.session = None;
        slot.generation = generation;
        self.free.push(id.handle.index);
        self.live -= 1;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), PlayModeError> {
        if self.closed {
            Err(PlayModeError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_resources(resources: PlayModeResources) -> Result<(), PlayModeError> {
    let engine = resources.world.engine;
    if resources.input_scope.engine != engine
        || resources.asset_scope.engine != engine
        || resources.render_target.engine != engine
        || !resources.input_scope.handle.is_valid()
        || !resources.asset_scope.handle.is_valid()
        || !resources.render_target.handle.is_valid()
        || resources.render_target.kind != ControlKind::RenderTarget
    {
        return Err(PlayModeError::InvalidResources);
    }
    Ok(())
}

fn validate_policy(policy: &PlayModePolicy, config: &PlayModeConfig) -> Result<(), PlayModeError> {
    if policy.writeback_fields.len() > config.max_writeback_fields {
        return Err(PlayModeError::WritebackFieldCapacity);
    }
    let mut fields = BTreeSet::new();
    let mut ranges: BTreeMap<u32, Vec<(usize, usize)>> = BTreeMap::new();
    for field in &policy.writeback_fields {
        if field.component == 0 || field.width == 0 || !fields.insert(*field) {
            return Err(PlayModeError::InvalidWritebackField);
        }
        let end = field
            .offset
            .checked_add(field.width)
            .ok_or(PlayModeError::InvalidWritebackField)?;
        ranges
            .entry(field.component)
            .or_default()
            .push((field.offset, end));
    }
    for component_ranges in ranges.values_mut() {
        component_ranges.sort_unstable();
        if component_ranges
            .windows(2)
            .any(|pair| pair[0].1 > pair[1].0)
        {
            return Err(PlayModeError::OverlappingWritebackField);
        }
    }
    Ok(())
}

fn build_writeback_commands(
    session: &PlayModeSession,
    authoring: &World,
) -> Result<Vec<WorldCommand>, PlayModeError> {
    let mut patches: BTreeMap<(crate::world::WorldEntity, u32), Vec<u8>> = BTreeMap::new();
    for field in &session.policy.writeback_fields {
        for (stable_key, baseline_components) in &session.baseline {
            let authoring_entity = authoring
                .entity_for_stable_key(*stable_key)
                .ok_or(PlayModeError::MissingStableObject)?;
            let play_entity = session
                .play_world
                .entity_for_stable_key(*stable_key)
                .ok_or(PlayModeError::MissingStableObject)?;
            let baseline = baseline_components
                .get(&field.component)
                .ok_or(PlayModeError::ComponentMissing)?;
            let current = authoring
                .component(authoring_entity, field.component)
                .ok_or(PlayModeError::ComponentMissing)?;
            let played = session
                .play_world
                .component(play_entity, field.component)
                .ok_or(PlayModeError::ComponentMissing)?;
            let end = field
                .offset
                .checked_add(field.width)
                .ok_or(PlayModeError::ComponentShape)?;
            let baseline_field = baseline
                .get(field.offset..end)
                .ok_or(PlayModeError::ComponentShape)?;
            let played_field = played
                .get(field.offset..end)
                .ok_or(PlayModeError::ComponentShape)?;
            if baseline_field == played_field {
                continue;
            }
            let patch = patches
                .entry((authoring_entity, field.component))
                .or_insert_with(|| current.to_vec());
            let target = patch
                .get_mut(field.offset..end)
                .ok_or(PlayModeError::ComponentShape)?;
            target.copy_from_slice(played_field);
        }
    }
    Ok(patches
        .into_iter()
        .filter_map(|((entity, component), value)| {
            (authoring.component(entity, component) != Some(value.as_slice())).then_some(
                WorldCommand::SetComponent {
                    entity,
                    component,
                    value,
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn authoring_world() -> World {
        World::new(
            WorldId {
                engine: handle(1),
                handle: handle(2),
            },
            WorldConfig::default(),
        )
        .expect("world")
    }

    fn resources(engine: EngineId) -> PlayModeResources {
        PlayModeResources {
            world: WorldId {
                engine,
                handle: handle(1),
            },
            input_scope: InputScopeId {
                engine,
                handle: handle(2),
            },
            render_target: StableControlRef {
                engine,
                kind: ControlKind::RenderTarget,
                handle: handle(3),
            },
            asset_scope: AssetScopeId {
                engine,
                handle: handle(4),
            },
        }
    }

    fn spawn(world: &mut World, stable_key: u64, values: [i64; 2]) {
        let mut component = Vec::new();
        component.extend_from_slice(&values[0].to_le_bytes());
        component.extend_from_slice(&values[1].to_le_bytes());
        world
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key,
                components: BTreeMap::from([(7, component)]),
            }])
            .expect("spawn");
    }

    fn set_values(world: &mut World, stable_key: u64, values: [i64; 2]) {
        let entity = world.entity_for_stable_key(stable_key).expect("entity");
        let mut component = Vec::new();
        component.extend_from_slice(&values[0].to_le_bytes());
        component.extend_from_slice(&values[1].to_le_bytes());
        world
            .apply_stage(vec![WorldCommand::SetComponent {
                entity,
                component: 7,
                value: component,
            }])
            .expect("set");
    }

    fn values(world: &World, stable_key: u64) -> [i64; 2] {
        let entity = world.entity_for_stable_key(stable_key).expect("entity");
        let component = world.component(entity, 7).expect("component");
        [
            i64::from_le_bytes(component[0..8].try_into().unwrap()),
            i64::from_le_bytes(component[8..16].try_into().unwrap()),
        ]
    }

    fn policy() -> PlayModePolicy {
        PlayModePolicy {
            writeback_fields: vec![AuthoringField {
                component: 7,
                offset: 0,
                width: 8,
            }],
        }
    }

    #[test]
    fn two_preview_engines_are_isolated_from_authoring_and_each_other() {
        let mut authoring = authoring_world();
        spawn(&mut authoring, 1, [10, 20]);
        let mut manager =
            PlayModeManager::new(handle(1), PlayModeConfig::default()).expect("manager");
        let first = manager
            .start(
                &authoring,
                resources(handle(10)),
                WorldConfig::default(),
                policy(),
            )
            .expect("first");
        let second = manager
            .start(
                &authoring,
                resources(handle(11)),
                WorldConfig::default(),
                policy(),
            )
            .expect("second");
        set_values(
            manager.play_world_mut(first.id).expect("first world"),
            1,
            [30, 40],
        );
        assert_eq!(values(&authoring, 1), [10, 20]);
        assert_eq!(
            values(manager.play_world(second.id).expect("second world"), 1),
            [10, 20]
        );
        assert_ne!(
            manager
                .play_world(first.id)
                .expect("first world")
                .entity_for_stable_key(1),
            manager
                .play_world(second.id)
                .expect("second world")
                .entity_for_stable_key(1)
        );
    }

    #[test]
    fn explicit_writeback_applies_only_whitelisted_authoring_field() {
        let mut authoring = authoring_world();
        spawn(&mut authoring, 1, [10, 20]);
        let source_revision = authoring.revision();
        let mut manager =
            PlayModeManager::new(handle(1), PlayModeConfig::default()).expect("manager");
        let started = manager
            .start(
                &authoring,
                resources(handle(10)),
                WorldConfig::default(),
                policy(),
            )
            .expect("start");
        set_values(
            manager.play_world_mut(started.id).expect("play world"),
            1,
            [30, 40],
        );
        let report = manager
            .exit(
                started.id,
                &mut authoring,
                PlayModeExit::ApplyAuthoring {
                    expected_authoring_revision: source_revision,
                },
            )
            .expect("exit");
        assert_eq!(report.changed_components, 1);
        assert_eq!(values(&authoring, 1), [30, 20]);
        assert_eq!(manager.live_sessions(), 0);
    }

    #[test]
    fn external_authoring_change_conflicts_and_keeps_play_session_alive() {
        let mut authoring = authoring_world();
        spawn(&mut authoring, 1, [10, 20]);
        let source_revision = authoring.revision();
        let mut manager =
            PlayModeManager::new(handle(1), PlayModeConfig::default()).expect("manager");
        let started = manager
            .start(
                &authoring,
                resources(handle(10)),
                WorldConfig::default(),
                policy(),
            )
            .expect("start");
        set_values(&mut authoring, 1, [11, 20]);
        assert_eq!(
            manager.exit(
                started.id,
                &mut authoring,
                PlayModeExit::ApplyAuthoring {
                    expected_authoring_revision: source_revision,
                }
            ),
            Err(PlayModeError::RevisionConflict)
        );
        assert_eq!(manager.live_sessions(), 1);
        manager
            .exit(started.id, &mut authoring, PlayModeExit::Discard)
            .expect("discard");
    }

    #[test]
    fn discard_reuses_slot_generation_and_rejects_stale_session() {
        let mut authoring = authoring_world();
        spawn(&mut authoring, 1, [10, 20]);
        let mut manager = PlayModeManager::new(
            handle(1),
            PlayModeConfig {
                max_sessions: 1,
                ..PlayModeConfig::default()
            },
        )
        .expect("manager");
        let first = manager
            .start(
                &authoring,
                resources(handle(10)),
                WorldConfig::default(),
                policy(),
            )
            .expect("first");
        assert_eq!(
            manager.start(
                &authoring,
                resources(handle(11)),
                WorldConfig::default(),
                policy(),
            ),
            Err(PlayModeError::SessionCapacity)
        );
        manager
            .exit(first.id, &mut authoring, PlayModeExit::Discard)
            .expect("discard");
        let second = manager
            .start(
                &authoring,
                resources(handle(11)),
                WorldConfig::default(),
                policy(),
            )
            .expect("second");
        assert_eq!(first.id.handle.index, second.id.handle.index);
        assert!(second.id.handle.generation > first.id.handle.generation);
        assert_eq!(
            manager.play_world(first.id).map(World::id),
            Err(PlayModeError::StaleSession)
        );
    }
}
