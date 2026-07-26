use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::Handle;

use crate::{
    asset::{AssetError, AssetScopeId, AssetScopeKind, AssetServer, AssetTicketResult},
    world::{World, WorldCommand, WorldEntity, WorldError, WorldId},
};

pub const SCENE_OBJECT_COMPONENT: u32 = 0xffff_ff00;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceAssetId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuthoringObjectId {
    pub source_asset: SourceAssetId,
    pub local_object_id: u64,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PrefabInstancePath(Vec<u64>);

impl PrefabInstancePath {
    pub fn new(segments: Vec<u64>) -> Result<Self, SceneError> {
        if segments.contains(&0) {
            return Err(SceneError::InvalidIdentity);
        }
        Ok(Self(segments))
    }

    pub const fn root() -> Self {
        Self(Vec::new())
    }

    pub fn segments(&self) -> &[u64] {
        &self.0
    }

    fn joined(&self, relative: &Self, max_depth: usize) -> Result<Self, SceneError> {
        let length = self
            .0
            .len()
            .checked_add(relative.0.len())
            .filter(|length| *length <= max_depth)
            .ok_or(SceneError::PathCapacity)?;
        let mut result = Vec::with_capacity(length);
        result.extend_from_slice(&self.0);
        result.extend_from_slice(&relative.0);
        Ok(Self(result))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SceneInstanceId {
    pub world: WorldId,
    pub handle: Handle,
}

impl SceneInstanceId {
    pub const fn is_valid(self) -> bool {
        self.world.is_valid() && self.handle.is_valid()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RuntimeObjectKey {
    pub root_scene_instance: SceneInstanceId,
    pub prefab_path: PrefabInstancePath,
    pub authoring: AuthoringObjectId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthoringObjectRef {
    PrefabRelative {
        relative_prefab_path: PrefabInstancePath,
        local_object_id: u64,
        expected_type: u64,
        required: bool,
    },
    SceneLocal {
        source_asset: SourceAssetId,
        local_object_id: u64,
        expected_type: u64,
        required: bool,
    },
    ExternalBinding {
        binding_key: u64,
        expected_type: u64,
        required: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoringFieldRef {
    pub component: u32,
    pub field: u32,
    pub target: AuthoringObjectRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneObjectSpec {
    pub key: RuntimeObjectKey,
    pub object_type: u64,
    pub schema_version: u32,
    pub partition: u64,
    pub components: BTreeMap<u32, BTreeMap<u32, Vec<u8>>>,
    pub references: Vec<AuthoringFieldRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrefabOverride {
    pub prefab_path: PrefabInstancePath,
    pub authoring: AuthoringObjectId,
    pub component: u32,
    pub field: u32,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneTransaction {
    pub source_revision: u64,
    pub objects: Vec<SceneObjectSpec>,
    pub overrides: Vec<PrefabOverride>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneMigrationError {
    Unsupported,
    InvalidSource,
    InvalidData,
    Capacity,
}

pub struct SceneMigrationInput<'a> {
    pub key: &'a RuntimeObjectKey,
    pub from_object_type: u64,
    pub from_schema_version: u32,
    pub current_blob: &'a [u8],
    pub target: &'a SceneObjectSpec,
}

pub trait SceneMigrationProvider {
    fn migrate(
        &self,
        input: SceneMigrationInput<'_>,
    ) -> Result<SceneObjectSpec, SceneMigrationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrefabRegistration {
    pub source_asset: SourceAssetId,
    pub source_revision: u64,
    pub schema_version: u32,
    pub dependencies: Vec<SourceAssetId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrefabRegistryConfig {
    pub max_prefabs: usize,
    pub max_dependencies_per_prefab: usize,
}

impl Default for PrefabRegistryConfig {
    fn default() -> Self {
        Self {
            max_prefabs: 100_000,
            max_dependencies_per_prefab: 256,
        }
    }
}

pub struct PrefabRegistry {
    config: PrefabRegistryConfig,
    entries: BTreeMap<SourceAssetId, PrefabRegistration>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrefabRegistryOwnerSnapshot {
    pub closed: bool,
    pub registrations: usize,
    pub dependency_edges: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrefabRegistryShutdownReport {
    pub released_registrations: usize,
    pub released_dependency_edges: usize,
}

impl PrefabRegistry {
    pub fn new(config: PrefabRegistryConfig) -> Result<Self, SceneError> {
        if config.max_prefabs == 0
            || config.max_prefabs > u32::MAX as usize
            || config.max_dependencies_per_prefab == 0
        {
            return Err(SceneError::InvalidConfig);
        }
        Ok(Self {
            config,
            entries: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn registration(&self, source: SourceAssetId) -> Option<&PrefabRegistration> {
        self.entries.get(&source)
    }

    pub fn owner_snapshot(&self) -> PrefabRegistryOwnerSnapshot {
        PrefabRegistryOwnerSnapshot {
            closed: self.closed,
            registrations: self.entries.len(),
            dependency_edges: self
                .entries
                .values()
                .map(|registration| registration.dependencies.len())
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> PrefabRegistryShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.entries.clear();
        PrefabRegistryShutdownReport {
            released_registrations: snapshot.registrations,
            released_dependency_edges: snapshot.dependency_edges,
        }
    }

    pub fn register_batch(
        &mut self,
        registrations: Vec<PrefabRegistration>,
    ) -> Result<(), SceneError> {
        self.ensure_open()?;
        let final_count = self
            .entries
            .len()
            .checked_add(registrations.len())
            .filter(|count| *count <= self.config.max_prefabs)
            .ok_or(SceneError::PrefabCapacity)?;
        let _ = final_count;
        let mut staged = self.entries.clone();
        for registration in registrations {
            validate_prefab_registration(&registration, &self.config)?;
            if staged
                .insert(registration.source_asset, registration)
                .is_some()
            {
                return Err(SceneError::DuplicatePrefab);
            }
        }
        validate_prefab_graph(&staged)?;
        self.entries = staged;
        Ok(())
    }

    pub fn hot_reload(&mut self, registration: PrefabRegistration) -> Result<(), SceneError> {
        self.ensure_open()?;
        validate_prefab_registration(&registration, &self.config)?;
        let current = self
            .entries
            .get(&registration.source_asset)
            .ok_or(SceneError::UnknownPrefab)?;
        if registration.source_revision <= current.source_revision {
            return Err(SceneError::StalePrefabRevision);
        }
        let mut staged = self.entries.clone();
        staged.insert(registration.source_asset, registration);
        validate_prefab_graph(&staged)?;
        self.entries = staged;
        Ok(())
    }

    fn validate_object_sources(
        &self,
        objects: &BTreeMap<RuntimeObjectKey, SceneObjectSpec>,
    ) -> Result<(), SceneError> {
        self.ensure_open()?;
        for object in objects.values() {
            if !object.key.prefab_path.0.is_empty()
                && !self
                    .entries
                    .contains_key(&object.key.authoring.source_asset)
            {
                return Err(SceneError::UnknownPrefab);
            }
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), SceneError> {
        if self.closed {
            Err(SceneError::Closed)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalObjectBinding {
    pub target: RuntimeObjectKey,
    pub object_type: u64,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneDiagnosticCode {
    OptionalReferenceUnresolved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneDiagnostic {
    pub source: RuntimeObjectKey,
    pub component: u32,
    pub field: u32,
    pub code: SceneDiagnosticCode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneApplyReport {
    pub source_revision: u64,
    pub mapping_revision: u64,
    pub created: usize,
    pub updated: usize,
    pub removed: usize,
    pub diagnostics: Vec<SceneDiagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneAssetScopeState {
    Detached,
    Open(AssetScopeId),
    Closing(AssetScopeId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneResolvedRefSnapshot {
    pub component: u32,
    pub field: u32,
    pub target: Option<RuntimeObjectKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneObjectSnapshot {
    pub key: RuntimeObjectKey,
    pub entity: WorldEntity,
    pub object_type: u64,
    pub schema_version: u32,
    pub blob: Vec<u8>,
    pub references: Vec<SceneResolvedRefSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneInstanceSnapshot {
    pub schema_version: u32,
    pub id: SceneInstanceId,
    pub source_revision: u64,
    pub mapping_revision: u64,
    pub objects: Vec<SceneObjectSnapshot>,
    pub diagnostics: Vec<SceneDiagnostic>,
    pub asset_scope: SceneAssetScopeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneConfig {
    pub max_objects: usize,
    pub max_prefab_depth: usize,
    pub max_components_per_object: usize,
    pub max_fields_per_component: usize,
    pub max_references_per_object: usize,
    pub max_overrides: usize,
    pub max_object_bytes: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            max_objects: 100_000,
            max_prefab_depth: 64,
            max_components_per_object: 256,
            max_fields_per_component: 1024,
            max_references_per_object: 1024,
            max_overrides: 100_000,
            max_object_bytes: 4 * 1024 * 1024,
            max_snapshot_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SceneError {
    Closed,
    InvalidConfig,
    WrongWorld,
    InvalidIdentity,
    StaleSourceRevision,
    ObjectCapacity,
    PathCapacity,
    ComponentCapacity,
    FieldCapacity,
    ReferenceCapacity,
    OverrideCapacity,
    ObjectByteCapacity,
    DuplicateObject,
    DuplicateStableKey,
    InvalidOverride,
    InvalidReference,
    RequiredReferenceUnresolved,
    TypeMismatch,
    MigrationRequired,
    MigrationFailed(SceneMigrationError),
    MigrationContract,
    MappingRevisionExhausted,
    StaleMappingRevision,
    UnknownObject,
    WorldDrift,
    SnapshotCapacity,
    SnapshotMalformed,
    RestoreRequiresEmpty,
    AssetScopeState,
    PrefabCapacity,
    DuplicatePrefab,
    UnknownPrefab,
    UnknownPrefabDependency,
    PrefabDependencyCycle,
    StalePrefabRevision,
    World(WorldError),
    Asset(AssetError),
}

impl From<WorldError> for SceneError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

impl From<AssetError> for SceneError {
    fn from(error: AssetError) -> Self {
        Self::Asset(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolvedFieldRef {
    component: u32,
    field: u32,
    target: Option<RuntimeObjectKey>,
}

#[derive(Clone, Debug)]
struct ObjectRecord {
    entity: WorldEntity,
    object_type: u64,
    schema_version: u32,
    blob: Vec<u8>,
    references: Vec<ResolvedFieldRef>,
}

#[derive(Clone, Debug)]
struct StagedObject {
    spec: SceneObjectSpec,
    stable_key: u64,
    blob: Vec<u8>,
    references: Vec<ResolvedFieldRef>,
}

pub struct SceneInstance {
    id: SceneInstanceId,
    config: SceneConfig,
    source_revision: u64,
    mapping_revision: u64,
    objects: BTreeMap<RuntimeObjectKey, ObjectRecord>,
    diagnostics: Vec<SceneDiagnostic>,
    asset_scope: SceneAssetScopeState,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneInstanceOwnerSnapshot {
    pub closed: bool,
    pub objects: usize,
    pub resolved_references: usize,
    pub diagnostic_count: usize,
    pub asset_scope: SceneAssetScopeState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneInstanceShutdownReport {
    pub released_objects: usize,
    pub released_references: usize,
    pub released_diagnostics: usize,
    pub asset_ticket_results: Vec<AssetTicketResult>,
}

impl SceneInstance {
    pub fn new(id: SceneInstanceId, config: SceneConfig) -> Result<Self, SceneError> {
        if !id.is_valid()
            || config.max_objects == 0
            || config.max_objects > u32::MAX as usize
            || config.max_prefab_depth == 0
            || config.max_prefab_depth > u32::MAX as usize
            || config.max_components_per_object == 0
            || config.max_components_per_object > u32::MAX as usize
            || config.max_fields_per_component == 0
            || config.max_fields_per_component > u32::MAX as usize
            || config.max_references_per_object == 0
            || config.max_references_per_object > u32::MAX as usize
            || config.max_overrides == 0
            || config.max_overrides > u32::MAX as usize
            || config.max_object_bytes == 0
            || config.max_snapshot_bytes == 0
        {
            return Err(SceneError::InvalidConfig);
        }
        Ok(Self {
            id,
            config,
            source_revision: 0,
            mapping_revision: 0,
            objects: BTreeMap::new(),
            diagnostics: Vec::new(),
            asset_scope: SceneAssetScopeState::Detached,
            closed: false,
        })
    }

    pub const fn id(&self) -> SceneInstanceId {
        self.id
    }

    pub const fn source_revision(&self) -> u64 {
        self.source_revision
    }

    pub const fn mapping_revision(&self) -> u64 {
        self.mapping_revision
    }

    pub fn diagnostics(&self) -> &[SceneDiagnostic] {
        &self.diagnostics
    }

    pub const fn asset_scope_state(&self) -> SceneAssetScopeState {
        self.asset_scope
    }

    pub fn owner_snapshot(&self) -> SceneInstanceOwnerSnapshot {
        SceneInstanceOwnerSnapshot {
            closed: self.closed,
            objects: self.objects.len(),
            resolved_references: self
                .objects
                .values()
                .map(|record| record.references.len())
                .sum(),
            diagnostic_count: self.diagnostics.len(),
            asset_scope: self.asset_scope,
        }
    }

    pub fn attach_asset_scope(
        &mut self,
        asset_server: &AssetServer,
        scope: AssetScopeId,
    ) -> Result<(), SceneError> {
        self.ensure_open()?;
        if self.asset_scope != SceneAssetScopeState::Detached {
            return Err(SceneError::AssetScopeState);
        }
        validate_asset_scope_state(
            self.id,
            SceneAssetScopeState::Open(scope),
            Some(asset_server),
        )?;
        self.asset_scope = SceneAssetScopeState::Open(scope);
        Ok(())
    }

    pub fn close_asset_scope(
        &mut self,
        asset_server: &mut AssetServer,
    ) -> Result<Vec<AssetTicketResult>, SceneError> {
        self.ensure_open()?;
        let SceneAssetScopeState::Open(scope) = self.asset_scope else {
            return Err(SceneError::AssetScopeState);
        };
        let results = asset_server.close_scope(scope)?;
        self.asset_scope = SceneAssetScopeState::Closing(scope);
        Ok(results)
    }

    pub fn release_asset_scope(
        &mut self,
        asset_server: &mut AssetServer,
    ) -> Result<(), SceneError> {
        self.ensure_open()?;
        let SceneAssetScopeState::Closing(scope) = self.asset_scope else {
            return Err(SceneError::AssetScopeState);
        };
        asset_server.release_scope(scope)?;
        self.asset_scope = SceneAssetScopeState::Detached;
        Ok(())
    }

    pub fn entity(&self, key: &RuntimeObjectKey) -> Option<WorldEntity> {
        self.objects.get(key).map(|record| record.entity)
    }

    pub fn cached_entity(
        &self,
        key: &RuntimeObjectKey,
        mapping_revision: u64,
    ) -> Result<WorldEntity, SceneError> {
        self.ensure_open()?;
        if mapping_revision != self.mapping_revision {
            return Err(SceneError::StaleMappingRevision);
        }
        self.entity(key).ok_or(SceneError::UnknownObject)
    }

    pub fn resolved_reference(
        &self,
        source: &RuntimeObjectKey,
        component: u32,
        field: u32,
    ) -> Result<Option<&RuntimeObjectKey>, SceneError> {
        self.ensure_open()?;
        let record = self.objects.get(source).ok_or(SceneError::UnknownObject)?;
        record
            .references
            .iter()
            .find(|reference| reference.component == component && reference.field == field)
            .map(|reference| reference.target.as_ref())
            .ok_or(SceneError::InvalidReference)
    }

    pub fn snapshot(
        &self,
        world: &World,
        asset_server: Option<&AssetServer>,
    ) -> Result<SceneInstanceSnapshot, SceneError> {
        self.ensure_open()?;
        let snapshot = SceneInstanceSnapshot {
            schema_version: 1,
            id: self.id,
            source_revision: self.source_revision,
            mapping_revision: self.mapping_revision,
            objects: self
                .objects
                .iter()
                .map(|(key, record)| SceneObjectSnapshot {
                    key: key.clone(),
                    entity: record.entity,
                    object_type: record.object_type,
                    schema_version: record.schema_version,
                    blob: record.blob.clone(),
                    references: record
                        .references
                        .iter()
                        .map(|reference| SceneResolvedRefSnapshot {
                            component: reference.component,
                            field: reference.field,
                            target: reference.target.clone(),
                        })
                        .collect(),
                })
                .collect(),
            diagnostics: self.diagnostics.clone(),
            asset_scope: self.asset_scope,
        };
        validate_scene_snapshot(&snapshot, self.id, &self.config, world, asset_server)?;
        Ok(snapshot)
    }

    pub fn restore(
        &mut self,
        snapshot: SceneInstanceSnapshot,
        world: &World,
        asset_server: Option<&AssetServer>,
    ) -> Result<(), SceneError> {
        self.ensure_open()?;
        if self.source_revision != 0
            || self.mapping_revision != 0
            || !self.objects.is_empty()
            || !self.diagnostics.is_empty()
            || self.asset_scope != SceneAssetScopeState::Detached
        {
            return Err(SceneError::RestoreRequiresEmpty);
        }
        validate_scene_snapshot(&snapshot, self.id, &self.config, world, asset_server)?;
        let mut objects = BTreeMap::new();
        for object in &snapshot.objects {
            objects.insert(
                object.key.clone(),
                ObjectRecord {
                    entity: object.entity,
                    object_type: object.object_type,
                    schema_version: object.schema_version,
                    blob: object.blob.clone(),
                    references: object
                        .references
                        .iter()
                        .map(|reference| ResolvedFieldRef {
                            component: reference.component,
                            field: reference.field,
                            target: reference.target.clone(),
                        })
                        .collect(),
                },
            );
        }
        self.source_revision = snapshot.source_revision;
        self.mapping_revision = snapshot.mapping_revision;
        self.objects = objects;
        self.diagnostics = snapshot.diagnostics;
        self.asset_scope = snapshot.asset_scope;
        Ok(())
    }

    pub fn apply(
        &mut self,
        world: &mut World,
        transaction: SceneTransaction,
        external_bindings: &BTreeMap<u64, ExternalObjectBinding>,
    ) -> Result<SceneApplyReport, SceneError> {
        self.apply_with_providers(world, transaction, external_bindings, None, None)
    }

    pub fn apply_with_providers(
        &mut self,
        world: &mut World,
        transaction: SceneTransaction,
        external_bindings: &BTreeMap<u64, ExternalObjectBinding>,
        prefab_registry: Option<&PrefabRegistry>,
        migration_provider: Option<&dyn SceneMigrationProvider>,
    ) -> Result<SceneApplyReport, SceneError> {
        self.ensure_open()?;
        if world.id() != self.id.world {
            return Err(SceneError::WrongWorld);
        }
        if transaction.source_revision <= self.source_revision {
            return Err(SceneError::StaleSourceRevision);
        }
        if transaction.objects.len() > self.config.max_objects {
            return Err(SceneError::ObjectCapacity);
        }
        if transaction.overrides.len() > self.config.max_overrides {
            return Err(SceneError::OverrideCapacity);
        }

        let mut desired = BTreeMap::new();
        for object in transaction.objects {
            validate_object_identity(self.id, &object, &self.config)?;
            let key = object.key.clone();
            if desired.insert(key, object).is_some() {
                return Err(SceneError::DuplicateObject);
            }
        }
        if let Some(registry) = prefab_registry {
            registry.validate_object_sources(&desired)?;
        }
        apply_migrations(
            &self.objects,
            &mut desired,
            migration_provider,
            &self.config,
        )?;
        apply_overrides(self.id, &mut desired, transaction.overrides, &self.config)?;
        let (resolved, diagnostics) =
            resolve_references(self.id, &desired, external_bindings, &self.config)?;

        let mut staged = BTreeMap::new();
        let mut stable_keys = BTreeSet::new();
        for (key, spec) in desired {
            let references = resolved.get(&key).cloned().unwrap_or_default();
            let blob = encode_object(&spec, &references, self.config.max_object_bytes)?;
            let stable_key = stable_key(&key)?;
            if !stable_keys.insert(stable_key) {
                return Err(SceneError::DuplicateStableKey);
            }
            staged.insert(
                key,
                StagedObject {
                    spec,
                    stable_key,
                    blob,
                    references,
                },
            );
        }

        let key_set_changed = self.objects.keys().ne(staged.keys());
        let mapping_revision = if key_set_changed {
            self.mapping_revision
                .checked_add(1)
                .ok_or(SceneError::MappingRevisionExhausted)?
        } else {
            self.mapping_revision
        };
        let mut commands = Vec::new();
        let mut removed = 0;
        let mut updated = 0;
        for (key, current) in &self.objects {
            if world.component(current.entity, SCENE_OBJECT_COMPONENT)
                != Some(current.blob.as_slice())
            {
                return Err(SceneError::WorldDrift);
            }
            match staged.get(key) {
                None => {
                    commands.push(WorldCommand::Despawn {
                        entity: current.entity,
                    });
                    removed += 1;
                }
                Some(next) => {
                    if next.blob != current.blob {
                        commands.push(WorldCommand::SetComponent {
                            entity: current.entity,
                            component: SCENE_OBJECT_COMPONENT,
                            value: next.blob.clone(),
                        });
                        updated += 1;
                    }
                }
            }
        }
        let mut created = 0;
        for (key, next) in &staged {
            if !self.objects.contains_key(key) {
                commands.push(WorldCommand::Spawn {
                    stable_key: next.stable_key,
                    components: BTreeMap::from([(SCENE_OBJECT_COMPONENT, next.blob.clone())]),
                });
                created += 1;
            }
        }

        let result = world.apply_stage(commands)?;
        let spawned = result.spawned.into_iter().collect::<BTreeMap<_, _>>();
        let mut next_objects = BTreeMap::new();
        for (key, next) in staged {
            let entity = self
                .objects
                .get(&key)
                .map(|record| record.entity)
                .or_else(|| spawned.get(&next.stable_key).copied())
                .ok_or(SceneError::WorldDrift)?;
            next_objects.insert(
                key,
                ObjectRecord {
                    entity,
                    object_type: next.spec.object_type,
                    schema_version: next.spec.schema_version,
                    blob: next.blob,
                    references: next.references,
                },
            );
        }
        self.objects = next_objects;
        self.source_revision = transaction.source_revision;
        self.mapping_revision = mapping_revision;
        self.diagnostics = diagnostics.clone();
        Ok(SceneApplyReport {
            source_revision: self.source_revision,
            mapping_revision,
            created,
            updated,
            removed,
            diagnostics,
        })
    }

    pub fn shutdown(
        &mut self,
        world: &mut World,
        asset_server: &mut AssetServer,
    ) -> Result<SceneInstanceShutdownReport, SceneError> {
        if self.closed {
            return Ok(SceneInstanceShutdownReport {
                released_objects: 0,
                released_references: 0,
                released_diagnostics: 0,
                asset_ticket_results: Vec::new(),
            });
        }
        if world.id() != self.id.world {
            return Err(SceneError::WrongWorld);
        }
        let snapshot = self.owner_snapshot();
        let despawns = self
            .objects
            .values()
            .filter(|record| world.is_live(record.entity))
            .map(|record| WorldCommand::Despawn {
                entity: record.entity,
            })
            .collect();
        world.apply_stage(despawns)?;

        let mut asset_ticket_results = Vec::new();
        if let SceneAssetScopeState::Open(scope) = self.asset_scope {
            asset_ticket_results = asset_server.close_scope(scope)?;
            self.asset_scope = SceneAssetScopeState::Closing(scope);
        }
        if let SceneAssetScopeState::Closing(scope) = self.asset_scope {
            asset_server.release_scope(scope)?;
            self.asset_scope = SceneAssetScopeState::Detached;
        }

        self.objects.clear();
        self.diagnostics.clear();
        self.closed = true;
        Ok(SceneInstanceShutdownReport {
            released_objects: snapshot.objects,
            released_references: snapshot.resolved_references,
            released_diagnostics: snapshot.diagnostic_count,
            asset_ticket_results,
        })
    }

    fn ensure_open(&self) -> Result<(), SceneError> {
        if self.closed {
            Err(SceneError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_object_identity(
    id: SceneInstanceId,
    object: &SceneObjectSpec,
    config: &SceneConfig,
) -> Result<(), SceneError> {
    if object.key.root_scene_instance != id
        || object.key.authoring.source_asset.0 == [0; 16]
        || object.key.authoring.local_object_id == 0
        || object.object_type == 0
        || object.schema_version == 0
        || object.partition == 0
    {
        return Err(SceneError::InvalidIdentity);
    }
    if object.key.prefab_path.0.len() > config.max_prefab_depth {
        return Err(SceneError::PathCapacity);
    }
    if object.components.len() > config.max_components_per_object {
        return Err(SceneError::ComponentCapacity);
    }
    if object.references.len() > config.max_references_per_object {
        return Err(SceneError::ReferenceCapacity);
    }
    for (component, fields) in &object.components {
        if *component == 0 || *component == SCENE_OBJECT_COMPONENT {
            return Err(SceneError::InvalidIdentity);
        }
        if fields.len() > config.max_fields_per_component {
            return Err(SceneError::FieldCapacity);
        }
        if fields.keys().any(|field| *field == 0) {
            return Err(SceneError::InvalidIdentity);
        }
    }
    Ok(())
}

fn validate_prefab_registration(
    registration: &PrefabRegistration,
    config: &PrefabRegistryConfig,
) -> Result<(), SceneError> {
    if registration.source_asset.0 == [0; 16]
        || registration.source_revision == 0
        || registration.schema_version == 0
        || registration.dependencies.len() > config.max_dependencies_per_prefab
    {
        return Err(SceneError::InvalidIdentity);
    }
    let mut dependencies = BTreeSet::new();
    for dependency in &registration.dependencies {
        if dependency.0 == [0; 16]
            || *dependency == registration.source_asset
            || !dependencies.insert(*dependency)
        {
            return Err(SceneError::PrefabDependencyCycle);
        }
    }
    Ok(())
}

fn validate_prefab_graph(
    entries: &BTreeMap<SourceAssetId, PrefabRegistration>,
) -> Result<(), SceneError> {
    for registration in entries.values() {
        if registration
            .dependencies
            .iter()
            .any(|dependency| !entries.contains_key(dependency))
        {
            return Err(SceneError::UnknownPrefabDependency);
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in entries.keys().copied() {
        if visited.contains(&root) {
            continue;
        }
        visiting.insert(root);
        let mut stack = vec![(root, 0_usize)];
        while let Some((source, dependency_index)) = stack.last_mut() {
            let dependencies = &entries[source].dependencies;
            if *dependency_index == dependencies.len() {
                let completed = *source;
                stack.pop();
                visiting.remove(&completed);
                visited.insert(completed);
                continue;
            }
            let dependency = dependencies[*dependency_index];
            *dependency_index += 1;
            if visited.contains(&dependency) {
                continue;
            }
            if !visiting.insert(dependency) {
                return Err(SceneError::PrefabDependencyCycle);
            }
            stack.push((dependency, 0));
        }
    }
    Ok(())
}

fn apply_migrations(
    current: &BTreeMap<RuntimeObjectKey, ObjectRecord>,
    desired: &mut BTreeMap<RuntimeObjectKey, SceneObjectSpec>,
    provider: Option<&dyn SceneMigrationProvider>,
    config: &SceneConfig,
) -> Result<(), SceneError> {
    for (key, target) in desired.iter_mut() {
        let Some(source) = current.get(key) else {
            continue;
        };
        if source.object_type == target.object_type
            && source.schema_version == target.schema_version
        {
            continue;
        }
        let provider = provider.ok_or(SceneError::MigrationRequired)?;
        let requested_key = target.key.clone();
        let requested_type = target.object_type;
        let requested_schema = target.schema_version;
        let requested_partition = target.partition;
        let migrated = provider
            .migrate(SceneMigrationInput {
                key,
                from_object_type: source.object_type,
                from_schema_version: source.schema_version,
                current_blob: &source.blob,
                target,
            })
            .map_err(SceneError::MigrationFailed)?;
        if migrated.key != requested_key
            || migrated.object_type != requested_type
            || migrated.schema_version != requested_schema
            || migrated.partition != requested_partition
        {
            return Err(SceneError::MigrationContract);
        }
        validate_object_identity(requested_key.root_scene_instance, &migrated, config)?;
        *target = migrated;
    }
    Ok(())
}

fn apply_overrides(
    id: SceneInstanceId,
    desired: &mut BTreeMap<RuntimeObjectKey, SceneObjectSpec>,
    overrides: Vec<PrefabOverride>,
    config: &SceneConfig,
) -> Result<(), SceneError> {
    let mut seen = BTreeSet::new();
    for item in overrides {
        if item.prefab_path.0.len() > config.max_prefab_depth
            || item.component == 0
            || item.field == 0
            || !seen.insert((
                item.prefab_path.clone(),
                item.authoring,
                item.component,
                item.field,
            ))
        {
            return Err(SceneError::InvalidOverride);
        }
        let key = RuntimeObjectKey {
            root_scene_instance: id,
            prefab_path: item.prefab_path,
            authoring: item.authoring,
        };
        let field = desired
            .get_mut(&key)
            .and_then(|object| object.components.get_mut(&item.component))
            .and_then(|fields| fields.get_mut(&item.field))
            .ok_or(SceneError::InvalidOverride)?;
        *field = item.value;
    }
    Ok(())
}

fn resolve_references(
    id: SceneInstanceId,
    desired: &BTreeMap<RuntimeObjectKey, SceneObjectSpec>,
    external: &BTreeMap<u64, ExternalObjectBinding>,
    config: &SceneConfig,
) -> Result<
    (
        BTreeMap<RuntimeObjectKey, Vec<ResolvedFieldRef>>,
        Vec<SceneDiagnostic>,
    ),
    SceneError,
> {
    let mut resolved = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (key, object) in desired {
        let mut fields = Vec::with_capacity(object.references.len());
        let mut seen = BTreeSet::new();
        for reference in &object.references {
            if reference.component == 0
                || reference.field == 0
                || !object
                    .components
                    .get(&reference.component)
                    .is_some_and(|component| component.contains_key(&reference.field))
                || !seen.insert((reference.component, reference.field))
            {
                return Err(SceneError::InvalidReference);
            }
            let (target, expected_type, required) = match &reference.target {
                AuthoringObjectRef::PrefabRelative {
                    relative_prefab_path,
                    local_object_id,
                    expected_type,
                    required,
                } => (
                    Some(RuntimeObjectKey {
                        root_scene_instance: id,
                        prefab_path: key
                            .prefab_path
                            .joined(relative_prefab_path, config.max_prefab_depth)?,
                        authoring: AuthoringObjectId {
                            source_asset: key.authoring.source_asset,
                            local_object_id: *local_object_id,
                        },
                    }),
                    *expected_type,
                    *required,
                ),
                AuthoringObjectRef::SceneLocal {
                    source_asset,
                    local_object_id,
                    expected_type,
                    required,
                } => (
                    Some(RuntimeObjectKey {
                        root_scene_instance: id,
                        prefab_path: PrefabInstancePath::root(),
                        authoring: AuthoringObjectId {
                            source_asset: *source_asset,
                            local_object_id: *local_object_id,
                        },
                    }),
                    *expected_type,
                    *required,
                ),
                AuthoringObjectRef::ExternalBinding {
                    binding_key,
                    expected_type,
                    required,
                } => {
                    if *binding_key == 0 {
                        return Err(SceneError::InvalidReference);
                    }
                    let binding = external.get(binding_key).filter(|binding| binding.visible);
                    if let Some(binding) = binding {
                        if binding.object_type != *expected_type
                            || !binding.target.root_scene_instance.is_valid()
                            || binding.target.authoring.source_asset.0 == [0; 16]
                            || binding.target.authoring.local_object_id == 0
                        {
                            return Err(SceneError::TypeMismatch);
                        }
                    }
                    (
                        binding.map(|binding| binding.target.clone()),
                        *expected_type,
                        *required,
                    )
                }
            };
            if expected_type == 0 {
                return Err(SceneError::InvalidReference);
            }
            let target = match target {
                Some(target) if target.root_scene_instance != id => Some(target),
                Some(target) => match desired.get(&target) {
                    Some(target_object) if target_object.object_type == expected_type => {
                        Some(target)
                    }
                    Some(_) => return Err(SceneError::TypeMismatch),
                    None => None,
                },
                None => None,
            };
            if target.is_none() {
                if required {
                    return Err(SceneError::RequiredReferenceUnresolved);
                }
                diagnostics.push(SceneDiagnostic {
                    source: key.clone(),
                    component: reference.component,
                    field: reference.field,
                    code: SceneDiagnosticCode::OptionalReferenceUnresolved,
                });
            }
            fields.push(ResolvedFieldRef {
                component: reference.component,
                field: reference.field,
                target,
            });
        }
        resolved.insert(key.clone(), fields);
    }
    Ok((resolved, diagnostics))
}

fn encode_object(
    object: &SceneObjectSpec,
    references: &[ResolvedFieldRef],
    max_bytes: usize,
) -> Result<Vec<u8>, SceneError> {
    let encoded_size = object_encoded_size(object, references)?;
    if encoded_size > max_bytes {
        return Err(SceneError::ObjectByteCapacity);
    }
    let mut bytes = Vec::with_capacity(encoded_size);
    bytes.extend_from_slice(b"VOSC");
    bytes.extend_from_slice(&object.object_type.to_le_bytes());
    bytes.extend_from_slice(&object.schema_version.to_le_bytes());
    bytes.extend_from_slice(&object.partition.to_le_bytes());
    bytes.extend_from_slice(&(object.components.len() as u32).to_le_bytes());
    for (component, fields) in &object.components {
        bytes.extend_from_slice(&component.to_le_bytes());
        bytes.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        for (field, value) in fields {
            bytes.extend_from_slice(&field.to_le_bytes());
            let length = u32::try_from(value.len()).map_err(|_| SceneError::ObjectByteCapacity)?;
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(value);
        }
    }
    bytes.extend_from_slice(&(references.len() as u32).to_le_bytes());
    for reference in references {
        bytes.extend_from_slice(&reference.component.to_le_bytes());
        bytes.extend_from_slice(&reference.field.to_le_bytes());
        match &reference.target {
            None => bytes.push(0),
            Some(target) => {
                bytes.push(1);
                encode_key(&mut bytes, target)?;
            }
        }
    }
    debug_assert_eq!(bytes.len(), encoded_size);
    Ok(bytes)
}

fn object_encoded_size(
    object: &SceneObjectSpec,
    references: &[ResolvedFieldRef],
) -> Result<usize, SceneError> {
    let mut size = 4_usize
        .checked_add(8)
        .and_then(|size| size.checked_add(4))
        .and_then(|size| size.checked_add(8))
        .and_then(|size| size.checked_add(4))
        .ok_or(SceneError::ObjectByteCapacity)?;
    for fields in object.components.values() {
        size = size.checked_add(8).ok_or(SceneError::ObjectByteCapacity)?;
        for value in fields.values() {
            size = size
                .checked_add(8)
                .and_then(|size| size.checked_add(value.len()))
                .ok_or(SceneError::ObjectByteCapacity)?;
        }
    }
    size = size.checked_add(4).ok_or(SceneError::ObjectByteCapacity)?;
    for reference in references {
        size = size.checked_add(9).ok_or(SceneError::ObjectByteCapacity)?;
        if let Some(target) = &reference.target {
            size = size
                .checked_add(encoded_key_size(target)?)
                .ok_or(SceneError::ObjectByteCapacity)?;
        }
    }
    Ok(size)
}

fn encoded_key_size(key: &RuntimeObjectKey) -> Result<usize, SceneError> {
    52_usize
        .checked_add(
            key.prefab_path
                .0
                .len()
                .checked_mul(8)
                .ok_or(SceneError::ObjectByteCapacity)?,
        )
        .ok_or(SceneError::ObjectByteCapacity)
}

fn encode_key(bytes: &mut Vec<u8>, key: &RuntimeObjectKey) -> Result<(), SceneError> {
    bytes.extend_from_slice(&key.root_scene_instance.world.engine.index.to_le_bytes());
    bytes.extend_from_slice(
        &key.root_scene_instance
            .world
            .engine
            .generation
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&key.root_scene_instance.world.handle.index.to_le_bytes());
    bytes.extend_from_slice(
        &key.root_scene_instance
            .world
            .handle
            .generation
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&key.root_scene_instance.handle.index.to_le_bytes());
    bytes.extend_from_slice(&key.root_scene_instance.handle.generation.to_le_bytes());
    let path_length =
        u32::try_from(key.prefab_path.0.len()).map_err(|_| SceneError::PathCapacity)?;
    bytes.extend_from_slice(&path_length.to_le_bytes());
    for segment in &key.prefab_path.0 {
        bytes.extend_from_slice(&segment.to_le_bytes());
    }
    bytes.extend_from_slice(&key.authoring.source_asset.0);
    bytes.extend_from_slice(&key.authoring.local_object_id.to_le_bytes());
    Ok(())
}

fn stable_key(key: &RuntimeObjectKey) -> Result<u64, SceneError> {
    let mut bytes = Vec::new();
    encode_key(&mut bytes, key)?;
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(if hash == 0 { 1 } else { hash })
}

fn validate_asset_scope_state(
    id: SceneInstanceId,
    state: SceneAssetScopeState,
    asset_server: Option<&AssetServer>,
) -> Result<(), SceneError> {
    let (scope, expected_open) = match state {
        SceneAssetScopeState::Detached => return Ok(()),
        SceneAssetScopeState::Open(scope) => (scope, true),
        SceneAssetScopeState::Closing(scope) => (scope, false),
    };
    if scope.engine != id.world.engine {
        return Err(SceneError::WrongWorld);
    }
    let server = asset_server.ok_or(SceneError::AssetScopeState)?;
    if server.scope_kind(scope) != Some(AssetScopeKind::Scene)
        || server.scope_is_open(scope) != Some(expected_open)
    {
        return Err(SceneError::AssetScopeState);
    }
    Ok(())
}

fn validate_scene_snapshot(
    snapshot: &SceneInstanceSnapshot,
    id: SceneInstanceId,
    config: &SceneConfig,
    world: &World,
    asset_server: Option<&AssetServer>,
) -> Result<(), SceneError> {
    if snapshot.schema_version != 1 || snapshot.id != id || world.id() != id.world {
        return Err(SceneError::SnapshotMalformed);
    }
    if snapshot.objects.len() > config.max_objects {
        return Err(SceneError::SnapshotCapacity);
    }
    if !snapshot.objects.is_empty()
        && (snapshot.source_revision == 0 || snapshot.mapping_revision == 0)
    {
        return Err(SceneError::SnapshotMalformed);
    }
    validate_asset_scope_state(id, snapshot.asset_scope, asset_server)?;
    let mut bytes = 48_usize;
    if bytes > config.max_snapshot_bytes {
        return Err(SceneError::SnapshotCapacity);
    }
    let mut keys = BTreeSet::new();
    for object in &snapshot.objects {
        validate_runtime_key(&object.key, Some(id), config.max_prefab_depth)?;
        if !keys.insert(object.key.clone())
            || object.entity.world != id.world
            || object.object_type == 0
            || object.schema_version == 0
            || object.blob.len() > config.max_object_bytes
            || object.references.len() > config.max_references_per_object
            || blob_object_type(&object.blob) != Some(object.object_type)
            || blob_schema_version(&object.blob) != Some(object.schema_version)
        {
            return Err(SceneError::SnapshotMalformed);
        }
        let expected_stable = stable_key(&object.key)?;
        if world.entity_for_stable_key(expected_stable) != Some(object.entity)
            || world.component(object.entity, SCENE_OBJECT_COMPONENT)
                != Some(object.blob.as_slice())
        {
            return Err(SceneError::WorldDrift);
        }
        bytes = snapshot_add(
            bytes,
            encoded_key_size(&object.key)?,
            config.max_snapshot_bytes,
        )?;
        bytes = snapshot_add(bytes, 28, config.max_snapshot_bytes)?;
        bytes = snapshot_add(bytes, object.blob.len(), config.max_snapshot_bytes)?;
        let mut fields = BTreeSet::new();
        for reference in &object.references {
            if reference.component == 0
                || reference.field == 0
                || !fields.insert((reference.component, reference.field))
            {
                return Err(SceneError::SnapshotMalformed);
            }
            bytes = snapshot_add(bytes, 9, config.max_snapshot_bytes)?;
            if let Some(target) = &reference.target {
                validate_runtime_key(target, None, config.max_prefab_depth)?;
                bytes = snapshot_add(bytes, encoded_key_size(target)?, config.max_snapshot_bytes)?;
            }
        }
    }
    for diagnostic in &snapshot.diagnostics {
        if !keys.contains(&diagnostic.source) || diagnostic.component == 0 || diagnostic.field == 0
        {
            return Err(SceneError::SnapshotMalformed);
        }
        bytes = snapshot_add(
            bytes,
            encoded_key_size(&diagnostic.source)?,
            config.max_snapshot_bytes,
        )?;
        bytes = snapshot_add(bytes, 9, config.max_snapshot_bytes)?;
    }
    let _ = bytes;
    Ok(())
}

fn validate_runtime_key(
    key: &RuntimeObjectKey,
    expected_scene: Option<SceneInstanceId>,
    max_depth: usize,
) -> Result<(), SceneError> {
    if !key.root_scene_instance.is_valid()
        || expected_scene.is_some_and(|scene| key.root_scene_instance != scene)
        || key.prefab_path.0.len() > max_depth
        || key.prefab_path.0.contains(&0)
        || key.authoring.source_asset.0 == [0; 16]
        || key.authoring.local_object_id == 0
    {
        return Err(SceneError::SnapshotMalformed);
    }
    Ok(())
}

fn snapshot_add(current: usize, add: usize, max: usize) -> Result<usize, SceneError> {
    current
        .checked_add(add)
        .filter(|bytes| *bytes <= max)
        .ok_or(SceneError::SnapshotCapacity)
}

fn blob_object_type(blob: &[u8]) -> Option<u64> {
    if blob.len() < 12 || &blob[..4] != b"VOSC" {
        return None;
    }
    Some(u64::from_le_bytes(blob[4..12].try_into().ok()?))
}

fn blob_schema_version(blob: &[u8]) -> Option<u32> {
    if blob.len() < 16 || &blob[..4] != b"VOSC" {
        return None;
    }
    Some(u32::from_le_bytes(blob[12..16].try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{ArtifactId, AssetId, AssetRegistration, AssetServerConfig};
    use crate::world::WorldConfig;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn world_id(engine: u32) -> WorldId {
        WorldId {
            engine: handle(engine),
            handle: handle(10),
        }
    }

    fn scene_id(engine: u32, scene: u32) -> SceneInstanceId {
        SceneInstanceId {
            world: world_id(engine),
            handle: handle(scene),
        }
    }

    fn source(value: u8) -> SourceAssetId {
        SourceAssetId([value; 16])
    }

    fn path(segments: &[u64]) -> PrefabInstancePath {
        PrefabInstancePath::new(segments.to_vec()).unwrap()
    }

    fn key(
        scene: SceneInstanceId,
        prefab_path: &[u64],
        source_asset: SourceAssetId,
        local: u64,
    ) -> RuntimeObjectKey {
        RuntimeObjectKey {
            root_scene_instance: scene,
            prefab_path: path(prefab_path),
            authoring: AuthoringObjectId {
                source_asset,
                local_object_id: local,
            },
        }
    }

    fn object(key: RuntimeObjectKey, object_type: u64, partition: u64) -> SceneObjectSpec {
        SceneObjectSpec {
            key,
            object_type,
            schema_version: 1,
            partition,
            components: BTreeMap::from([(
                1,
                BTreeMap::from([(1, b"base".to_vec()), (2, b"ref".to_vec())]),
            )]),
            references: Vec::new(),
        }
    }

    fn config() -> SceneConfig {
        SceneConfig {
            max_objects: 16,
            max_prefab_depth: 8,
            max_components_per_object: 4,
            max_fields_per_component: 8,
            max_references_per_object: 8,
            max_overrides: 8,
            max_object_bytes: 4096,
            max_snapshot_bytes: 16 * 1024,
        }
    }

    fn world(id: WorldId, max_entities: usize) -> World {
        World::new(
            id,
            WorldConfig {
                max_entities,
                max_components_per_entity: 4,
                max_component_bytes: 4096,
                max_snapshot_bytes: 4096,
            },
        )
        .unwrap()
    }

    fn asset_server(engine: u32) -> AssetServer {
        AssetServer::new(
            handle(engine),
            handle(99),
            AssetServerConfig {
                max_assets: 4,
                max_dependencies_per_asset: 4,
                max_scopes: 4,
                max_tickets: 4,
            },
        )
        .unwrap()
    }

    fn prefab_registration(asset: u8, revision: u64, dependencies: &[u8]) -> PrefabRegistration {
        PrefabRegistration {
            source_asset: source(asset),
            source_revision: revision,
            schema_version: 1,
            dependencies: dependencies
                .iter()
                .map(|dependency| source(*dependency))
                .collect(),
        }
    }

    struct GoodMigration;

    impl SceneMigrationProvider for GoodMigration {
        fn migrate(
            &self,
            input: SceneMigrationInput<'_>,
        ) -> Result<SceneObjectSpec, SceneMigrationError> {
            if !input.current_blob.starts_with(b"VOSC") {
                return Err(SceneMigrationError::InvalidSource);
            }
            let mut migrated = input.target.clone();
            *migrated
                .components
                .get_mut(&1)
                .and_then(|fields| fields.get_mut(&1))
                .ok_or(SceneMigrationError::InvalidData)? = b"migrated".to_vec();
            Ok(migrated)
        }
    }

    struct FailedMigration;

    impl SceneMigrationProvider for FailedMigration {
        fn migrate(
            &self,
            _input: SceneMigrationInput<'_>,
        ) -> Result<SceneObjectSpec, SceneMigrationError> {
            Err(SceneMigrationError::InvalidData)
        }
    }

    struct ContractBreakingMigration;

    impl SceneMigrationProvider for ContractBreakingMigration {
        fn migrate(
            &self,
            input: SceneMigrationInput<'_>,
        ) -> Result<SceneObjectSpec, SceneMigrationError> {
            let mut migrated = input.target.clone();
            migrated.schema_version += 1;
            Ok(migrated)
        }
    }

    #[test]
    fn repeated_and_nested_prefab_paths_produce_unique_runtime_identity_and_refs() {
        let id = scene_id(1, 20);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 8);
        let first = key(id, &[1], source(1), 1);
        let second = key(id, &[2], source(1), 1);
        let nested = key(id, &[1, 7], source(1), 2);
        let mut first_object = object(first.clone(), 10, 1);
        first_object.references.push(AuthoringFieldRef {
            component: 1,
            field: 2,
            target: AuthoringObjectRef::PrefabRelative {
                relative_prefab_path: path(&[7]),
                local_object_id: 2,
                expected_type: 20,
                required: true,
            },
        });
        let mut second_object = object(second.clone(), 10, 1);
        second_object.references.push(AuthoringFieldRef {
            component: 1,
            field: 2,
            target: AuthoringObjectRef::PrefabRelative {
                relative_prefab_path: path(&[99]),
                local_object_id: 2,
                expected_type: 20,
                required: false,
            },
        });
        let report = scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![first_object, second_object, object(nested.clone(), 20, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(report.created, 3);
        assert_eq!(report.mapping_revision, 1);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(
            scene.resolved_reference(&first, 1, 2).unwrap(),
            Some(&nested)
        );
        assert_eq!(scene.resolved_reference(&second, 1, 2).unwrap(), None);
        let entities = [
            scene.entity(&first).unwrap(),
            scene.entity(&second).unwrap(),
            scene.entity(&nested).unwrap(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(entities.len(), 3);
    }

    #[test]
    fn prefab_registry_batch_and_hot_reload_reject_unknown_cycle_and_stale_revision_atomically() {
        let mut registry = PrefabRegistry::new(PrefabRegistryConfig {
            max_prefabs: 4,
            max_dependencies_per_prefab: 4,
        })
        .unwrap();
        assert_eq!(
            registry.register_batch(vec![prefab_registration(1, 1, &[9])]),
            Err(SceneError::UnknownPrefabDependency)
        );
        assert_eq!(registry.registration(source(1)), None);
        registry
            .register_batch(vec![
                prefab_registration(1, 1, &[2]),
                prefab_registration(2, 1, &[]),
            ])
            .unwrap();
        assert_eq!(
            registry.hot_reload(prefab_registration(2, 2, &[1])),
            Err(SceneError::PrefabDependencyCycle)
        );
        assert_eq!(registry.registration(source(2)).unwrap().source_revision, 1);
        assert_eq!(
            registry.hot_reload(prefab_registration(2, 1, &[])),
            Err(SceneError::StalePrefabRevision)
        );

        let id = scene_id(1, 20);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 2);
        scene
            .apply_with_providers(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(key(id, &[1], source(1), 1), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
                Some(&registry),
                None,
            )
            .unwrap();
        let revision = world.revision();
        assert_eq!(
            scene.apply_with_providers(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![object(key(id, &[1], source(8), 1), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
                Some(&registry),
                None,
            ),
            Err(SceneError::UnknownPrefab)
        );
        assert_eq!(world.revision(), revision);
        assert_eq!(scene.source_revision(), 1);
    }

    #[test]
    fn schema_migration_provider_success_failure_and_contract_are_atomic() {
        let id = scene_id(1, 20);
        let object_key = key(id, &[], source(1), 1);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 2);
        scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(object_key.clone(), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        let entity = scene.entity(&object_key).unwrap();
        let initial_revision = world.revision();
        let mut schema_two = object(object_key.clone(), 10, 1);
        schema_two.schema_version = 2;
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![schema_two.clone()],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::MigrationRequired)
        );
        assert_eq!(world.revision(), initial_revision);
        scene
            .apply_with_providers(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![schema_two],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
                None,
                Some(&GoodMigration),
            )
            .unwrap();
        assert_eq!(scene.entity(&object_key), Some(entity));
        assert_eq!(
            blob_schema_version(scene.objects[&object_key].blob.as_slice()),
            Some(2)
        );

        let mut schema_three = object(object_key.clone(), 10, 1);
        schema_three.schema_version = 3;
        let migrated_revision = world.revision();
        assert_eq!(
            scene.apply_with_providers(
                &mut world,
                SceneTransaction {
                    source_revision: 3,
                    objects: vec![schema_three.clone()],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
                None,
                Some(&FailedMigration),
            ),
            Err(SceneError::MigrationFailed(
                SceneMigrationError::InvalidData
            ))
        );
        assert_eq!(world.revision(), migrated_revision);
        assert_eq!(scene.source_revision(), 2);
        assert_eq!(
            scene.apply_with_providers(
                &mut world,
                SceneTransaction {
                    source_revision: 3,
                    objects: vec![schema_three],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
                None,
                Some(&ContractBreakingMigration),
            ),
            Err(SceneError::MigrationContract)
        );
        assert_eq!(world.revision(), migrated_revision);
        assert_eq!(scene.source_revision(), 2);
    }

    #[test]
    fn required_optional_and_visible_external_bindings_are_resolved_in_staging() {
        let id = scene_id(1, 20);
        let external_scene = scene_id(1, 21);
        let external_target = key(external_scene, &[], source(9), 1);
        let source_key = key(id, &[], source(1), 1);
        let mut source_object = object(source_key.clone(), 10, 1);
        source_object.references.push(AuthoringFieldRef {
            component: 1,
            field: 2,
            target: AuthoringObjectRef::ExternalBinding {
                binding_key: 77,
                expected_type: 55,
                required: true,
            },
        });
        let bindings = BTreeMap::from([(
            77,
            ExternalObjectBinding {
                target: external_target.clone(),
                object_type: 55,
                visible: true,
            },
        )]);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 4);
        scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![source_object.clone()],
                    overrides: Vec::new(),
                },
                &bindings,
            )
            .unwrap();
        assert_eq!(
            scene.resolved_reference(&source_key, 1, 2).unwrap(),
            Some(&external_target)
        );

        let invisible = BTreeMap::from([(
            77,
            ExternalObjectBinding {
                target: external_target,
                object_type: 55,
                visible: false,
            },
        )]);
        let world_revision = world.revision();
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![source_object],
                    overrides: Vec::new(),
                },
                &invisible,
            ),
            Err(SceneError::RequiredReferenceUnresolved)
        );
        assert_eq!(world.revision(), world_revision);
        assert_eq!(scene.source_revision(), 1);
    }

    #[test]
    fn override_partition_unload_reload_and_mapping_revision_are_consistent() {
        let id = scene_id(1, 20);
        let first = key(id, &[1], source(1), 1);
        let second = key(id, &[2], source(1), 1);
        let first_spec = object(first.clone(), 10, 1);
        let second_spec = object(second.clone(), 10, 2);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 4);
        scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![first_spec.clone(), second_spec.clone()],
                    overrides: vec![PrefabOverride {
                        prefab_path: path(&[1]),
                        authoring: first.authoring,
                        component: 1,
                        field: 1,
                        value: b"override".to_vec(),
                    }],
                },
                &BTreeMap::new(),
            )
            .unwrap();
        let first_entity = scene.entity(&first).unwrap();
        let old_second = scene.entity(&second).unwrap();
        let old_mapping = scene.mapping_revision();

        let unload = scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![first_spec.clone()],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!((unload.updated, unload.removed), (1, 1));
        assert_eq!(scene.entity(&first), Some(first_entity));
        assert_eq!(scene.entity(&second), None);
        assert_eq!(
            scene.cached_entity(&first, old_mapping),
            Err(SceneError::StaleMappingRevision)
        );

        let reload = scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 3,
                    objects: vec![first_spec.clone(), second_spec],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(reload.created, 1);
        let new_second = scene.entity(&second).unwrap();
        assert_eq!(new_second.entity.index, old_second.entity.index);
        assert_ne!(new_second.entity.generation, old_second.entity.generation);

        let world_revision = world.revision();
        let mapping_revision = scene.mapping_revision();
        let no_op = scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 4,
                    objects: vec![first_spec, object(second, 10, 2)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(no_op.mapping_revision, mapping_revision);
        assert_eq!(world.revision(), world_revision);
    }

    #[test]
    fn reference_override_type_and_world_capacity_failures_leave_both_owners_unchanged() {
        let id = scene_id(1, 20);
        let first = key(id, &[], source(1), 1);
        let second = key(id, &[], source(1), 2);
        let mut required = object(first.clone(), 10, 1);
        required.references.push(AuthoringFieldRef {
            component: 1,
            field: 2,
            target: AuthoringObjectRef::SceneLocal {
                source_asset: source(8),
                local_object_id: 99,
                expected_type: 20,
                required: true,
            },
        });
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 1);
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![required],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::RequiredReferenceUnresolved)
        );
        assert_eq!(
            (
                scene.source_revision(),
                scene.mapping_revision(),
                world.revision()
            ),
            (0, 0, 0)
        );

        assert!(matches!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(first.clone(), 10, 1), object(second, 20, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::World(WorldError::EntityCapacity))
        ));
        assert_eq!(
            (
                scene.source_revision(),
                scene.mapping_revision(),
                world.revision()
            ),
            (0, 0, 0)
        );

        scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(first.clone(), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        let revision = world.revision();
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![object(first.clone(), 11, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::MigrationRequired)
        );
        assert_eq!(world.revision(), revision);
        assert_eq!(scene.source_revision(), 1);
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![object(first.clone(), 10, 1)],
                    overrides: vec![PrefabOverride {
                        prefab_path: PrefabInstancePath::root(),
                        authoring: first.authoring,
                        component: 9,
                        field: 1,
                        value: Vec::new(),
                    }],
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::InvalidOverride)
        );
        assert_eq!(world.revision(), revision);
        assert_eq!(scene.source_revision(), 1);
    }

    #[test]
    fn byte_preflight_mapping_exhaustion_and_world_drift_fail_before_scene_commit() {
        let id = scene_id(1, 20);
        let first = key(id, &[], source(1), 1);
        let second = key(id, &[], source(1), 2);

        let mut tiny_config = config();
        tiny_config.max_object_bytes = 16;
        let mut tiny_scene = SceneInstance::new(id, tiny_config).unwrap();
        let mut tiny_world = world(id.world, 2);
        assert_eq!(
            tiny_scene.apply(
                &mut tiny_world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(first.clone(), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::ObjectByteCapacity)
        );
        assert_eq!(tiny_world.revision(), 0);

        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut world = world(id.world, 3);
        scene
            .apply(
                &mut world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(first.clone(), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        scene.mapping_revision = u64::MAX;
        let revision = world.revision();
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![object(first.clone(), 10, 1), object(second.clone(), 10, 1),],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::MappingRevisionExhausted)
        );
        assert_eq!(world.revision(), revision);
        assert_eq!(scene.source_revision(), 1);

        scene.mapping_revision = 1;
        let entity = scene.entity(&first).unwrap();
        world
            .apply_stage(vec![WorldCommand::SetComponent {
                entity,
                component: SCENE_OBJECT_COMPONENT,
                value: b"foreign-mutation".to_vec(),
            }])
            .unwrap();
        let drift_revision = world.revision();
        assert_eq!(
            scene.apply(
                &mut world,
                SceneTransaction {
                    source_revision: 2,
                    objects: vec![object(first, 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            ),
            Err(SceneError::WorldDrift)
        );
        assert_eq!(world.revision(), drift_revision);
        assert_eq!(scene.source_revision(), 1);
        assert_eq!(scene.entity(&second), None);
    }

    #[test]
    fn world_and_scene_snapshots_restore_exact_mapping_refs_and_asset_scope() {
        let id = scene_id(1, 20);
        let first = key(id, &[1], source(1), 1);
        let nested = key(id, &[1, 7], source(1), 2);
        let mut first_spec = object(first.clone(), 10, 1);
        first_spec.references.push(AuthoringFieldRef {
            component: 1,
            field: 2,
            target: AuthoringObjectRef::PrefabRelative {
                relative_prefab_path: path(&[7]),
                local_object_id: 2,
                expected_type: 20,
                required: true,
            },
        });
        let mut source_world = world(id.world, 4);
        let mut source_scene = SceneInstance::new(id, config()).unwrap();
        let mut assets = asset_server(1);
        let scope = assets.create_scope(AssetScopeKind::Scene).unwrap();
        source_scene.attach_asset_scope(&assets, scope).unwrap();
        source_scene
            .apply(
                &mut source_world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![first_spec, object(nested.clone(), 20, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        let expected_entity = source_scene.entity(&first).unwrap();
        let world_snapshot = source_world.snapshot().unwrap();
        let scene_snapshot = source_scene.snapshot(&source_world, Some(&assets)).unwrap();

        let mut restored_world = world(id.world, 4);
        restored_world.restore(world_snapshot).unwrap();
        let mut restored_scene = SceneInstance::new(id, config()).unwrap();
        restored_scene
            .restore(scene_snapshot, &restored_world, Some(&assets))
            .unwrap();
        assert_eq!(restored_scene.entity(&first), Some(expected_entity));
        assert_eq!(
            restored_scene.resolved_reference(&first, 1, 2).unwrap(),
            Some(&nested)
        );
        assert_eq!(restored_scene.source_revision(), 1);
        assert_eq!(restored_scene.mapping_revision(), 1);
        assert_eq!(
            restored_scene.asset_scope_state(),
            SceneAssetScopeState::Open(scope)
        );
    }

    #[test]
    fn malformed_or_cross_owner_scene_snapshot_leaves_restore_target_empty() {
        let id = scene_id(1, 20);
        let first = key(id, &[], source(1), 1);
        let mut source_world = world(id.world, 2);
        let mut source_scene = SceneInstance::new(id, config()).unwrap();
        source_scene
            .apply(
                &mut source_world,
                SceneTransaction {
                    source_revision: 1,
                    objects: vec![object(first.clone(), 10, 1)],
                    overrides: Vec::new(),
                },
                &BTreeMap::new(),
            )
            .unwrap();
        let mut malformed = source_scene.snapshot(&source_world, None).unwrap();
        malformed.objects[0].entity.entity.generation += 1;
        let mut target = SceneInstance::new(id, config()).unwrap();
        assert_eq!(
            target.restore(malformed, &source_world, None),
            Err(SceneError::WorldDrift)
        );
        assert_eq!(
            (target.source_revision(), target.mapping_revision()),
            (0, 0)
        );
        assert_eq!(target.entity(&first), None);

        let mut cross_owner = source_scene.snapshot(&source_world, None).unwrap();
        cross_owner.id = scene_id(1, 21);
        assert_eq!(
            target.restore(cross_owner, &source_world, None),
            Err(SceneError::SnapshotMalformed)
        );
        assert_eq!(
            (target.source_revision(), target.mapping_revision()),
            (0, 0)
        );

        let mut tiny = config();
        tiny.max_snapshot_bytes = 32;
        let tiny_scene = SceneInstance::new(id, tiny).unwrap();
        assert_eq!(
            tiny_scene.snapshot(&source_world, None),
            Err(SceneError::SnapshotCapacity)
        );
    }

    #[test]
    fn scene_asset_scope_closes_tickets_before_scope_release_and_rejects_wrong_kind() {
        let id = scene_id(1, 20);
        let mut scene = SceneInstance::new(id, config()).unwrap();
        let mut assets = asset_server(1);
        let wrong_kind = assets.create_scope(AssetScopeKind::Mode).unwrap();
        assert_eq!(
            scene.attach_asset_scope(&assets, wrong_kind),
            Err(SceneError::AssetScopeState)
        );
        let scope = assets.create_scope(AssetScopeKind::Scene).unwrap();
        scene.attach_asset_scope(&assets, scope).unwrap();
        let asset_ref = assets
            .register_batch(vec![AssetRegistration {
                asset_id: AssetId([1; 16]),
                asset_type: 7,
                source_revision: 1,
                artifact_id: ArtifactId([2; 16]),
                dependencies: Vec::new(),
            }])
            .unwrap()[0];
        let ticket = assets.request(scope, asset_ref, 100).unwrap();
        let terminal = scene.close_asset_scope(&mut assets).unwrap();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].ticket, ticket);
        assert_eq!(
            scene.asset_scope_state(),
            SceneAssetScopeState::Closing(scope)
        );
        assert_eq!(
            scene.release_asset_scope(&mut assets),
            Err(SceneError::Asset(AssetError::ScopeInUse))
        );
        assets.release_ticket(ticket).unwrap();
        scene.release_asset_scope(&mut assets).unwrap();
        assert_eq!(scene.asset_scope_state(), SceneAssetScopeState::Detached);
        assert_eq!(assets.scope_kind(scope), None);
    }
}
