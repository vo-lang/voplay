use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, EntityId};

use crate::world::{World, WorldCommand, WorldEntity, WorldError, WorldId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionConfig {
    pub max_schemas: usize,
    pub max_fields_per_schema: usize,
    pub max_schema_bytes: usize,
    pub max_page_entities: usize,
    pub max_page_bytes: usize,
    pub max_edits_per_transaction: usize,
    pub max_edit_bytes: usize,
    pub max_history_entries: usize,
    pub max_history_bytes: usize,
    pub max_pending_steps: u32,
}

impl Default for InspectionConfig {
    fn default() -> Self {
        Self {
            max_schemas: 4096,
            max_fields_per_schema: 256,
            max_schema_bytes: 4 * 1024 * 1024,
            max_page_entities: 1024,
            max_page_bytes: 16 * 1024 * 1024,
            max_edits_per_transaction: 1024,
            max_edit_bytes: 4 * 1024 * 1024,
            max_history_entries: 1024,
            max_history_bytes: 64 * 1024 * 1024,
            max_pending_steps: 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionFieldSchema {
    pub field: u32,
    pub name: String,
    pub offset: usize,
    pub width: usize,
    pub editable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionComponentSchema {
    pub component: u32,
    pub type_id: u64,
    pub version: u32,
    pub name: String,
    pub fields: Vec<InspectionFieldSchema>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionExecutionState {
    Running,
    Paused,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectedComponent {
    pub component: u32,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectedEntity {
    pub entity: WorldEntity,
    pub stable_key: u64,
    pub components: Vec<InspectedComponent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionWorldPage {
    pub world: WorldId,
    pub revision: u64,
    pub source_revision: u64,
    pub schema_fingerprint: u64,
    pub component_schemas: Vec<InspectionComponentSchema>,
    pub entities: Vec<InspectedEntity>,
    pub next_cursor: Option<usize>,
    pub encoded_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionFieldEdit {
    pub entity: WorldEntity,
    pub component: u32,
    pub field: u32,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionEditTransaction {
    pub engine: EngineId,
    pub transaction_id: u64,
    pub expected_world_revision: u64,
    pub edits: Vec<InspectionFieldEdit>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionEditResult {
    pub transaction_id: u64,
    pub world_revision: u64,
    pub changed_components: usize,
    pub history_recorded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionHistoryResult {
    pub transaction_id: u64,
    pub world_revision: u64,
    pub changed_components: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InspectionError {
    Closed,
    InvalidConfig,
    WrongEngine,
    WrongWorld,
    RegistryFrozen,
    RegistryNotFrozen,
    SchemaCapacity,
    FieldCapacity,
    SchemaByteCapacity,
    InvalidSchema,
    DuplicateSchema,
    DuplicateField,
    OverlappingField,
    UnknownSchema,
    UnknownField,
    FieldReadOnly,
    FieldWidth,
    PageCapacity,
    InvalidCursor,
    InvalidTransaction,
    DuplicateTransaction,
    TransactionCapacity,
    EditByteCapacity,
    DuplicateEdit,
    RevisionConflict,
    ComponentMissing,
    ComponentShape,
    HistoryCapacity,
    NothingToUndo,
    NothingToRedo,
    InvalidExecutionState,
    StepCapacity,
    RevisionExhausted,
    World(WorldError),
}

impl From<WorldError> for InspectionError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

pub struct InspectionSchemaRegistry {
    engine: EngineId,
    config: InspectionConfig,
    schemas: BTreeMap<u32, InspectionComponentSchema>,
    schema_bytes: usize,
    frozen: bool,
    fingerprint: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionSchemaOwnerSnapshot {
    pub closed: bool,
    pub schemas: usize,
    pub fields: usize,
    pub schema_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionSchemaShutdownReport {
    pub released_schemas: usize,
    pub released_fields: usize,
    pub released_schema_bytes: usize,
}

impl InspectionSchemaRegistry {
    pub fn new(engine: EngineId, config: InspectionConfig) -> Result<Self, InspectionError> {
        validate_config(config)?;
        if !engine.is_valid() {
            return Err(InspectionError::WrongEngine);
        }
        Ok(Self {
            engine,
            config,
            schemas: BTreeMap::new(),
            schema_bytes: 0,
            frozen: false,
            fingerprint: 0,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn owner_snapshot(&self) -> InspectionSchemaOwnerSnapshot {
        InspectionSchemaOwnerSnapshot {
            closed: self.closed,
            schemas: self.schemas.len(),
            fields: self
                .schemas
                .values()
                .map(|schema| schema.fields.len())
                .sum(),
            schema_bytes: self.schema_bytes,
        }
    }

    pub fn shutdown(&mut self) -> InspectionSchemaShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.schemas.clear();
        self.schema_bytes = 0;
        self.frozen = false;
        self.fingerprint = 0;
        InspectionSchemaShutdownReport {
            released_schemas: snapshot.schemas,
            released_fields: snapshot.fields,
            released_schema_bytes: snapshot.schema_bytes,
        }
    }

    pub fn register(
        &mut self,
        mut schema: InspectionComponentSchema,
    ) -> Result<(), InspectionError> {
        self.ensure_open()?;
        if self.frozen {
            return Err(InspectionError::RegistryFrozen);
        }
        if schema.component == 0
            || schema.type_id == 0
            || schema.version == 0
            || schema.name.is_empty()
            || schema.fields.is_empty()
        {
            return Err(InspectionError::InvalidSchema);
        }
        if self.schemas.len() == self.config.max_schemas {
            return Err(InspectionError::SchemaCapacity);
        }
        if schema.fields.len() > self.config.max_fields_per_schema {
            return Err(InspectionError::FieldCapacity);
        }
        if self.schemas.contains_key(&schema.component) {
            return Err(InspectionError::DuplicateSchema);
        }
        schema.fields.sort_by_key(|field| field.field);
        let mut field_ids = BTreeSet::new();
        let mut field_names = BTreeSet::new();
        let mut ranges = Vec::with_capacity(schema.fields.len());
        let mut bytes = schema.name.len();
        for field in &schema.fields {
            if field.field == 0 || field.name.is_empty() || field.width == 0 {
                return Err(InspectionError::InvalidSchema);
            }
            if !field_ids.insert(field.field) || !field_names.insert(field.name.as_str()) {
                return Err(InspectionError::DuplicateField);
            }
            let end = field
                .offset
                .checked_add(field.width)
                .ok_or(InspectionError::InvalidSchema)?;
            ranges.push((field.offset, end));
            bytes = bytes
                .checked_add(field.name.len())
                .and_then(|bytes| bytes.checked_add(32))
                .ok_or(InspectionError::SchemaByteCapacity)?;
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(InspectionError::OverlappingField);
        }
        let schema_bytes = self
            .schema_bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= self.config.max_schema_bytes)
            .ok_or(InspectionError::SchemaByteCapacity)?;
        self.schemas.insert(schema.component, schema);
        self.schema_bytes = schema_bytes;
        Ok(())
    }

    pub fn freeze(&mut self) -> Result<u64, InspectionError> {
        self.ensure_open()?;
        if self.frozen {
            return Ok(self.fingerprint);
        }
        if self.schemas.is_empty() {
            return Err(InspectionError::InvalidSchema);
        }
        let mut hash = 0xcbf29ce484222325_u64;
        for schema in self.schemas.values() {
            hash_value(&mut hash, &schema.component.to_le_bytes());
            hash_value(&mut hash, &schema.type_id.to_le_bytes());
            hash_value(&mut hash, &schema.version.to_le_bytes());
            hash_value(&mut hash, schema.name.as_bytes());
            for field in &schema.fields {
                hash_value(&mut hash, &field.field.to_le_bytes());
                hash_value(&mut hash, field.name.as_bytes());
                hash_value(&mut hash, &(field.offset as u64).to_le_bytes());
                hash_value(&mut hash, &(field.width as u64).to_le_bytes());
                hash_value(&mut hash, &[u8::from(field.editable)]);
            }
        }
        self.fingerprint = hash;
        self.frozen = true;
        Ok(hash)
    }

    pub const fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub const fn fingerprint(&self) -> Option<u64> {
        if self.frozen {
            Some(self.fingerprint)
        } else {
            None
        }
    }

    pub fn schemas(&self) -> impl Iterator<Item = &InspectionComponentSchema> {
        self.schemas.values()
    }

    fn field(&self, component: u32, field: u32) -> Result<&InspectionFieldSchema, InspectionError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(InspectionError::RegistryNotFrozen);
        }
        self.schemas
            .get(&component)
            .ok_or(InspectionError::UnknownSchema)?
            .fields
            .iter()
            .find(|schema| schema.field == field)
            .ok_or(InspectionError::UnknownField)
    }

    fn ensure_open(&self) -> Result<(), InspectionError> {
        if self.closed {
            Err(InspectionError::Closed)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
struct ComponentPatch {
    entity: WorldEntity,
    component: u32,
    before: Vec<u8>,
    after: Vec<u8>,
}

#[derive(Clone, Debug)]
struct EditRecord {
    transaction_id: u64,
    expected_revision: u64,
    bytes: usize,
    patches: Vec<ComponentPatch>,
}

pub struct InspectionService {
    engine: EngineId,
    config: InspectionConfig,
    last_transaction_id: u64,
    undo: Vec<EditRecord>,
    redo: Vec<EditRecord>,
    history_bytes: usize,
    execution: InspectionExecutionState,
    control_revision: u64,
    pending_steps: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionShutdownReport {
    pub released_undo_records: usize,
    pub released_redo_records: usize,
    pub released_history_bytes: usize,
    pub cancelled_steps: u32,
}

impl InspectionService {
    pub fn new(engine: EngineId, config: InspectionConfig) -> Result<Self, InspectionError> {
        validate_config(config)?;
        if !engine.is_valid() {
            return Err(InspectionError::WrongEngine);
        }
        Ok(Self {
            engine,
            config,
            last_transaction_id: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            history_bytes: 0,
            execution: InspectionExecutionState::Running,
            control_revision: 0,
            pending_steps: 0,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn execution_state(&self) -> InspectionExecutionState {
        self.execution
    }

    pub const fn control_revision(&self) -> u64 {
        self.control_revision
    }

    pub const fn pending_steps(&self) -> u32 {
        self.pending_steps
    }

    pub const fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub const fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn shutdown(&mut self) -> InspectionShutdownReport {
        let report = InspectionShutdownReport {
            released_undo_records: self.undo.len(),
            released_redo_records: self.redo.len(),
            released_history_bytes: self.history_bytes,
            cancelled_steps: self.pending_steps,
        };
        self.undo.clear();
        self.redo.clear();
        self.history_bytes = 0;
        self.pending_steps = 0;
        report
    }

    pub fn inspect_world(
        &self,
        world: &World,
        schemas: &InspectionSchemaRegistry,
        cursor: usize,
        limit: usize,
    ) -> Result<InspectionWorldPage, InspectionError> {
        self.validate_world_and_schemas(world, schemas)?;
        if limit == 0 || limit > self.config.max_page_entities {
            return Err(InspectionError::PageCapacity);
        }
        let snapshot = world.snapshot()?;
        if cursor > snapshot.slots.len() {
            return Err(InspectionError::InvalidCursor);
        }
        let mut entities = Vec::new();
        let mut encoded_bytes = 0_usize;
        let mut index = cursor;
        while index < snapshot.slots.len() && entities.len() < limit {
            let slot = &snapshot.slots[index];
            index += 1;
            let Some(stable_key) = slot.stable_key else {
                continue;
            };
            let entity_bytes = slot
                .components
                .iter()
                .try_fold(24_usize, |bytes, (_, value)| {
                    bytes.checked_add(8)?.checked_add(value.len())
                });
            let next_bytes = entity_bytes
                .and_then(|bytes| encoded_bytes.checked_add(bytes))
                .filter(|bytes| *bytes <= self.config.max_page_bytes)
                .ok_or(InspectionError::PageCapacity)?;
            let components = slot
                .components
                .iter()
                .map(|(component, value)| InspectedComponent {
                    component: *component,
                    value: value.clone(),
                })
                .collect();
            entities.push(InspectedEntity {
                entity: WorldEntity {
                    world: world.id(),
                    entity: EntityId {
                        index: (index - 1) as u32,
                        generation: slot.generation,
                    },
                },
                stable_key,
                components,
            });
            encoded_bytes = next_bytes;
        }
        let component_schemas = page_component_schemas(schemas, &entities);
        encoded_bytes = encoded_bytes
            .checked_add(component_schemas_bytes(&component_schemas)?)
            .filter(|bytes| *bytes <= self.config.max_page_bytes)
            .ok_or(InspectionError::PageCapacity)?;
        Ok(InspectionWorldPage {
            world: world.id(),
            revision: snapshot.revision,
            source_revision: snapshot.revision,
            schema_fingerprint: schemas
                .fingerprint()
                .ok_or(InspectionError::RegistryNotFrozen)?,
            component_schemas,
            entities,
            next_cursor: (index < snapshot.slots.len()).then_some(index),
            encoded_bytes,
        })
    }

    pub fn inspect_entities(
        &self,
        world: &World,
        schemas: &InspectionSchemaRegistry,
        requested: &[WorldEntity],
        source_revision: u64,
        cursor: usize,
        limit: usize,
    ) -> Result<InspectionWorldPage, InspectionError> {
        self.validate_world_and_schemas(world, schemas)?;
        if limit == 0 || limit > self.config.max_page_entities {
            return Err(InspectionError::PageCapacity);
        }
        let requested = requested.iter().copied().collect::<BTreeSet<_>>();
        if requested
            .iter()
            .any(|entity| entity.world != world.id() || !world.is_live(*entity))
        {
            return Err(InspectionError::WrongWorld);
        }
        let requested = requested.into_iter().collect::<Vec<_>>();
        if cursor > requested.len() {
            return Err(InspectionError::InvalidCursor);
        }
        let snapshot = world.snapshot()?;
        let mut entities = Vec::new();
        let mut encoded_bytes = 0usize;
        let mut index = cursor;
        while index < requested.len() && entities.len() < limit {
            let entity = requested[index];
            index += 1;
            let slot = snapshot
                .slots
                .get(entity.entity.index as usize)
                .filter(|slot| {
                    slot.generation == entity.entity.generation && slot.stable_key.is_some()
                })
                .ok_or(InspectionError::WrongWorld)?;
            let entity_bytes = slot
                .components
                .iter()
                .try_fold(24_usize, |bytes, (_, value)| {
                    bytes.checked_add(8)?.checked_add(value.len())
                });
            encoded_bytes = entity_bytes
                .and_then(|bytes| encoded_bytes.checked_add(bytes))
                .filter(|bytes| *bytes <= self.config.max_page_bytes)
                .ok_or(InspectionError::PageCapacity)?;
            entities.push(InspectedEntity {
                entity,
                stable_key: slot.stable_key.unwrap(),
                components: slot
                    .components
                    .iter()
                    .map(|(component, value)| InspectedComponent {
                        component: *component,
                        value: value.clone(),
                    })
                    .collect(),
            });
        }
        let component_schemas = page_component_schemas(schemas, &entities);
        encoded_bytes = encoded_bytes
            .checked_add(component_schemas_bytes(&component_schemas)?)
            .filter(|bytes| *bytes <= self.config.max_page_bytes)
            .ok_or(InspectionError::PageCapacity)?;
        Ok(InspectionWorldPage {
            world: world.id(),
            revision: snapshot.revision,
            source_revision,
            schema_fingerprint: schemas
                .fingerprint()
                .ok_or(InspectionError::RegistryNotFrozen)?,
            component_schemas,
            entities,
            next_cursor: (index < requested.len()).then_some(index),
            encoded_bytes,
        })
    }

    pub fn apply_edit_at_stage(
        &mut self,
        world: &mut World,
        schemas: &InspectionSchemaRegistry,
        transaction: InspectionEditTransaction,
    ) -> Result<InspectionEditResult, InspectionError> {
        self.validate_world_and_schemas(world, schemas)?;
        if transaction.engine != self.engine {
            return Err(InspectionError::WrongEngine);
        }
        if transaction.transaction_id == 0 || transaction.edits.is_empty() {
            return Err(InspectionError::InvalidTransaction);
        }
        if transaction.transaction_id <= self.last_transaction_id {
            return Err(InspectionError::DuplicateTransaction);
        }
        if transaction.expected_world_revision != world.revision() {
            return Err(InspectionError::RevisionConflict);
        }
        if transaction.edits.len() > self.config.max_edits_per_transaction {
            return Err(InspectionError::TransactionCapacity);
        }
        let edit_bytes = transaction
            .edits
            .iter()
            .try_fold(0_usize, |bytes, edit| bytes.checked_add(edit.value.len()));
        if edit_bytes.is_none_or(|bytes| bytes > self.config.max_edit_bytes) {
            return Err(InspectionError::EditByteCapacity);
        }
        let mut seen = BTreeSet::new();
        let mut components: BTreeMap<(WorldEntity, u32), (Vec<u8>, Vec<u8>)> = BTreeMap::new();
        for edit in &transaction.edits {
            if edit.entity.world != world.id() {
                return Err(InspectionError::WrongWorld);
            }
            if !seen.insert((edit.entity, edit.component, edit.field)) {
                return Err(InspectionError::DuplicateEdit);
            }
            let field = schemas.field(edit.component, edit.field)?;
            if !field.editable {
                return Err(InspectionError::FieldReadOnly);
            }
            if edit.value.len() != field.width {
                return Err(InspectionError::FieldWidth);
            }
            let current = world
                .component(edit.entity, edit.component)
                .ok_or(InspectionError::ComponentMissing)?;
            let entry = components
                .entry((edit.entity, edit.component))
                .or_insert_with(|| (current.to_vec(), current.to_vec()));
            let end = field
                .offset
                .checked_add(field.width)
                .ok_or(InspectionError::ComponentShape)?;
            let target = entry
                .1
                .get_mut(field.offset..end)
                .ok_or(InspectionError::ComponentShape)?;
            target.copy_from_slice(&edit.value);
        }
        let patches = components
            .into_iter()
            .filter_map(|((entity, component), (before, after))| {
                (before != after).then_some(ComponentPatch {
                    entity,
                    component,
                    before,
                    after,
                })
            })
            .collect::<Vec<_>>();
        let history_bytes = patch_bytes(&patches)?;
        let redo_bytes = self
            .redo
            .iter()
            .try_fold(0_usize, |bytes, record| bytes.checked_add(record.bytes));
        let redo_bytes = redo_bytes.ok_or(InspectionError::HistoryCapacity)?;
        let retained_bytes = self.history_bytes.saturating_sub(redo_bytes);
        if !patches.is_empty()
            && (self.undo.len() + 1 > self.config.max_history_entries
                || retained_bytes
                    .checked_add(history_bytes)
                    .is_none_or(|bytes| bytes > self.config.max_history_bytes))
        {
            return Err(InspectionError::HistoryCapacity);
        }
        let commands = patches
            .iter()
            .map(|patch| WorldCommand::SetComponent {
                entity: patch.entity,
                component: patch.component,
                value: patch.after.clone(),
            })
            .collect();
        let result = world.apply_stage(commands)?;
        self.last_transaction_id = transaction.transaction_id;
        self.history_bytes = retained_bytes;
        self.redo.clear();
        if patches.is_empty() {
            return Ok(InspectionEditResult {
                transaction_id: transaction.transaction_id,
                world_revision: result.revision,
                changed_components: 0,
                history_recorded: false,
            });
        }
        self.history_bytes = self
            .history_bytes
            .checked_add(history_bytes)
            .ok_or(InspectionError::HistoryCapacity)?;
        let changed_components = patches.len();
        self.undo.push(EditRecord {
            transaction_id: transaction.transaction_id,
            expected_revision: result.revision,
            bytes: history_bytes,
            patches,
        });
        Ok(InspectionEditResult {
            transaction_id: transaction.transaction_id,
            world_revision: result.revision,
            changed_components,
            history_recorded: true,
        })
    }

    pub fn undo(&mut self, world: &mut World) -> Result<InspectionHistoryResult, InspectionError> {
        self.validate_world(world)?;
        let record = self.undo.last().ok_or(InspectionError::NothingToUndo)?;
        if world.revision() != record.expected_revision {
            return Err(InspectionError::RevisionConflict);
        }
        let commands = record
            .patches
            .iter()
            .map(|patch| WorldCommand::SetComponent {
                entity: patch.entity,
                component: patch.component,
                value: patch.before.clone(),
            })
            .collect();
        let result = world.apply_stage(commands)?;
        let mut record = self.undo.pop().ok_or(InspectionError::NothingToUndo)?;
        record.expected_revision = result.revision;
        let changed_components = record.patches.len();
        let transaction_id = record.transaction_id;
        self.redo.push(record);
        Ok(InspectionHistoryResult {
            transaction_id,
            world_revision: result.revision,
            changed_components,
        })
    }

    pub fn redo(&mut self, world: &mut World) -> Result<InspectionHistoryResult, InspectionError> {
        self.validate_world(world)?;
        let record = self.redo.last().ok_or(InspectionError::NothingToRedo)?;
        if world.revision() != record.expected_revision {
            return Err(InspectionError::RevisionConflict);
        }
        let commands = record
            .patches
            .iter()
            .map(|patch| WorldCommand::SetComponent {
                entity: patch.entity,
                component: patch.component,
                value: patch.after.clone(),
            })
            .collect();
        let result = world.apply_stage(commands)?;
        let mut record = self.redo.pop().ok_or(InspectionError::NothingToRedo)?;
        record.expected_revision = result.revision;
        let changed_components = record.patches.len();
        let transaction_id = record.transaction_id;
        self.undo.push(record);
        Ok(InspectionHistoryResult {
            transaction_id,
            world_revision: result.revision,
            changed_components,
        })
    }

    pub fn pause(&mut self, expected_revision: u64) -> Result<u64, InspectionError> {
        if expected_revision != self.control_revision {
            return Err(InspectionError::RevisionConflict);
        }
        if self.execution != InspectionExecutionState::Running {
            return Err(InspectionError::InvalidExecutionState);
        }
        self.advance_control_revision()?;
        self.execution = InspectionExecutionState::Paused;
        Ok(self.control_revision)
    }

    pub fn request_steps(
        &mut self,
        expected_revision: u64,
        steps: u32,
    ) -> Result<u64, InspectionError> {
        if expected_revision != self.control_revision {
            return Err(InspectionError::RevisionConflict);
        }
        if self.execution != InspectionExecutionState::Paused || steps == 0 {
            return Err(InspectionError::InvalidExecutionState);
        }
        let pending = self
            .pending_steps
            .checked_add(steps)
            .filter(|pending| *pending <= self.config.max_pending_steps)
            .ok_or(InspectionError::StepCapacity)?;
        self.advance_control_revision()?;
        self.pending_steps = pending;
        Ok(self.control_revision)
    }

    pub fn take_step(&mut self) -> bool {
        if self.execution != InspectionExecutionState::Paused || self.pending_steps == 0 {
            return false;
        }
        self.pending_steps -= 1;
        true
    }

    pub fn resume(&mut self, expected_revision: u64) -> Result<u64, InspectionError> {
        if expected_revision != self.control_revision {
            return Err(InspectionError::RevisionConflict);
        }
        if self.execution != InspectionExecutionState::Paused || self.pending_steps != 0 {
            return Err(InspectionError::InvalidExecutionState);
        }
        self.advance_control_revision()?;
        self.execution = InspectionExecutionState::Running;
        Ok(self.control_revision)
    }

    fn validate_world_and_schemas(
        &self,
        world: &World,
        schemas: &InspectionSchemaRegistry,
    ) -> Result<(), InspectionError> {
        self.validate_world(world)?;
        if schemas.engine != self.engine {
            return Err(InspectionError::WrongEngine);
        }
        if !schemas.is_frozen() {
            return Err(InspectionError::RegistryNotFrozen);
        }
        Ok(())
    }

    fn validate_world(&self, world: &World) -> Result<(), InspectionError> {
        if world.id().engine != self.engine {
            return Err(InspectionError::WrongEngine);
        }
        Ok(())
    }

    fn advance_control_revision(&mut self) -> Result<(), InspectionError> {
        self.control_revision = self
            .control_revision
            .checked_add(1)
            .ok_or(InspectionError::RevisionExhausted)?;
        Ok(())
    }
}

fn page_component_schemas(
    schemas: &InspectionSchemaRegistry,
    entities: &[InspectedEntity],
) -> Vec<InspectionComponentSchema> {
    let components = entities
        .iter()
        .flat_map(|entity| {
            entity
                .components
                .iter()
                .map(|component| component.component)
        })
        .collect::<BTreeSet<_>>();
    schemas
        .schemas()
        .filter(|schema| components.contains(&schema.component))
        .cloned()
        .collect()
}

fn component_schemas_bytes(
    schemas: &[InspectionComponentSchema],
) -> Result<usize, InspectionError> {
    schemas.iter().try_fold(0usize, |bytes, schema| {
        schema.fields.iter().try_fold(
            bytes
                .checked_add(24)
                .and_then(|bytes| bytes.checked_add(schema.name.len()))
                .ok_or(InspectionError::PageCapacity)?,
            |bytes, field| {
                bytes
                    .checked_add(32)
                    .and_then(|bytes| bytes.checked_add(field.name.len()))
                    .ok_or(InspectionError::PageCapacity)
            },
        )
    })
}

fn validate_config(config: InspectionConfig) -> Result<(), InspectionError> {
    if config.max_schemas == 0
        || config.max_fields_per_schema == 0
        || config.max_schema_bytes == 0
        || config.max_page_entities == 0
        || config.max_page_bytes == 0
        || config.max_edits_per_transaction == 0
        || config.max_edit_bytes == 0
        || config.max_history_entries == 0
        || config.max_history_bytes == 0
        || config.max_pending_steps == 0
    {
        return Err(InspectionError::InvalidConfig);
    }
    Ok(())
}

fn patch_bytes(patches: &[ComponentPatch]) -> Result<usize, InspectionError> {
    patches.iter().try_fold(0_usize, |bytes, patch| {
        bytes
            .checked_add(patch.before.len())
            .and_then(|bytes| bytes.checked_add(patch.after.len()))
            .and_then(|bytes| bytes.checked_add(24))
            .ok_or(InspectionError::HistoryCapacity)
    })
}

fn hash_value(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
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

    fn engine() -> EngineId {
        handle(1)
    }

    fn world() -> World {
        World::new(
            WorldId {
                engine: engine(),
                handle: handle(2),
            },
            crate::world::WorldConfig::default(),
        )
        .expect("world")
    }

    fn schema(component: u32, name: &str) -> InspectionComponentSchema {
        InspectionComponentSchema {
            component,
            type_id: 100 + component as u64,
            version: 1,
            name: name.to_owned(),
            fields: vec![
                InspectionFieldSchema {
                    field: 1,
                    name: "x".to_owned(),
                    offset: 0,
                    width: 2,
                    editable: true,
                },
                InspectionFieldSchema {
                    field: 2,
                    name: "locked".to_owned(),
                    offset: 2,
                    width: 2,
                    editable: false,
                },
            ],
        }
    }

    fn registry(order: &[u32]) -> InspectionSchemaRegistry {
        let mut registry =
            InspectionSchemaRegistry::new(engine(), InspectionConfig::default()).expect("registry");
        for component in order {
            registry
                .register(schema(*component, &format!("component-{component}")))
                .expect("schema");
        }
        registry.freeze().expect("freeze");
        registry
    }

    fn spawn(world: &mut World, stable_key: u64, value: [u8; 4]) -> WorldEntity {
        let mut components = BTreeMap::new();
        components.insert(1, value.to_vec());
        world
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key,
                components,
            }])
            .expect("spawn")
            .spawned[0]
            .1
    }

    #[test]
    fn schema_fingerprint_is_stable_and_freeze_rejects_mutation() {
        let first = registry(&[1, 2]);
        let second = registry(&[2, 1]);
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.schemas().count(), 2);

        let mut frozen = first;
        assert_eq!(
            frozen.register(schema(3, "late")),
            Err(InspectionError::RegistryFrozen)
        );
        let mut overlapping = schema(3, "overlap");
        overlapping.fields[1].offset = 1;
        let mut fresh =
            InspectionSchemaRegistry::new(engine(), InspectionConfig::default()).expect("fresh");
        assert_eq!(
            fresh.register(overlapping),
            Err(InspectionError::OverlappingField)
        );
    }

    #[test]
    fn world_pages_preserve_owner_revision_cursor_and_byte_budget() {
        let schemas = registry(&[1]);
        let mut world = world();
        let first = spawn(&mut world, 10, [1, 2, 3, 4]);
        spawn(&mut world, 20, [5, 6, 7, 8]);
        let service = InspectionService::new(
            engine(),
            InspectionConfig {
                max_page_entities: 1,
                ..InspectionConfig::default()
            },
        )
        .expect("service");
        let page = service
            .inspect_world(&world, &schemas, 0, 1)
            .expect("first page");
        assert_eq!(page.revision, world.revision());
        assert_eq!(page.entities[0].entity, first);
        assert_eq!(page.entities[0].stable_key, 10);
        let next = page.next_cursor.expect("next cursor");
        let second = service
            .inspect_world(&world, &schemas, next, 1)
            .expect("second page");
        assert_eq!(second.entities[0].stable_key, 20);
        assert_eq!(second.next_cursor, None);

        let tiny = InspectionService::new(
            engine(),
            InspectionConfig {
                max_page_bytes: 1,
                ..InspectionConfig::default()
            },
        )
        .expect("tiny");
        assert_eq!(
            tiny.inspect_world(&world, &schemas, 0, 1),
            Err(InspectionError::PageCapacity)
        );
    }

    #[test]
    fn edit_undo_redo_and_external_revision_conflict_are_atomic() {
        let schemas = registry(&[1]);
        let mut world = world();
        let entity = spawn(&mut world, 10, [1, 2, 3, 4]);
        let mut service =
            InspectionService::new(engine(), InspectionConfig::default()).expect("service");
        let applied = service
            .apply_edit_at_stage(
                &mut world,
                &schemas,
                InspectionEditTransaction {
                    engine: engine(),
                    transaction_id: 1,
                    expected_world_revision: 1,
                    edits: vec![InspectionFieldEdit {
                        entity,
                        component: 1,
                        field: 1,
                        value: vec![9, 8],
                    }],
                },
            )
            .expect("apply");
        assert_eq!(applied.world_revision, 2);
        assert_eq!(world.component(entity, 1), Some(&[9, 8, 3, 4][..]));
        assert_eq!(service.undo_depth(), 1);
        assert_eq!(service.undo(&mut world).expect("undo").world_revision, 3);
        assert_eq!(world.component(entity, 1), Some(&[1, 2, 3, 4][..]));
        assert_eq!(service.redo(&mut world).expect("redo").world_revision, 4);
        assert_eq!(world.component(entity, 1), Some(&[9, 8, 3, 4][..]));

        world
            .apply_stage(vec![WorldCommand::SetComponent {
                entity,
                component: 1,
                value: vec![7, 7, 7, 7],
            }])
            .expect("external edit");
        assert_eq!(
            service.undo(&mut world),
            Err(InspectionError::RevisionConflict)
        );
        assert_eq!(service.undo_depth(), 1);
        assert_eq!(world.component(entity, 1), Some(&[7, 7, 7, 7][..]));

        let revision = world.revision();
        assert_eq!(
            service.apply_edit_at_stage(
                &mut world,
                &schemas,
                InspectionEditTransaction {
                    engine: engine(),
                    transaction_id: 2,
                    expected_world_revision: revision,
                    edits: vec![InspectionFieldEdit {
                        entity,
                        component: 1,
                        field: 2,
                        value: vec![0, 0],
                    }],
                },
            ),
            Err(InspectionError::FieldReadOnly)
        );
        assert_eq!(world.revision(), revision);
    }

    #[test]
    fn pause_step_resume_uses_expected_control_revision_and_bounded_queue() {
        let mut service = InspectionService::new(
            engine(),
            InspectionConfig {
                max_pending_steps: 2,
                ..InspectionConfig::default()
            },
        )
        .expect("service");
        assert_eq!(service.pause(0), Ok(1));
        assert_eq!(service.request_steps(1, 2), Ok(2));
        assert_eq!(
            service.request_steps(2, 1),
            Err(InspectionError::StepCapacity)
        );
        assert!(service.take_step());
        assert!(service.take_step());
        assert!(!service.take_step());
        assert_eq!(service.resume(2), Ok(3));
        assert_eq!(service.execution_state(), InspectionExecutionState::Running);
        assert_eq!(service.pause(2), Err(InspectionError::RevisionConflict));
    }
}
