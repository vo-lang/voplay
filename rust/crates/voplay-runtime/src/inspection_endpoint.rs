use voplay_protocol::{EngineId, EntityId};

use crate::{
    inspection::{
        InspectionEditTransaction, InspectionError, InspectionFieldEdit, InspectionHistoryResult,
        InspectionSchemaRegistry, InspectionService, InspectionWorldPage,
    },
    world::{World, WorldEntity},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionRequestControl {
    None,
    Pause { expected_revision: u64 },
    Step { expected_revision: u64, steps: u32 },
    Resume { expected_revision: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionEndpointReply {
    pub payload: Vec<u8>,
    pub control: InspectionRequestControl,
}

impl InspectionEndpointReply {
    pub fn finish_control(&mut self, revision: u64) {
        if self.control != InspectionRequestControl::None && self.payload.len() == 10 {
            self.payload[2..10].copy_from_slice(&revision.to_le_bytes());
        }
    }

    pub fn reject_control(&mut self, error: InspectionError) {
        self.payload = encode_error(error);
        self.control = InspectionRequestControl::None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionEndpointError {
    Malformed,
    Capacity,
    Closed,
}

pub struct InspectionEndpointRuntime {
    engine: EngineId,
    schemas: InspectionSchemaRegistry,
    service: InspectionService,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionEndpointOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub pending_steps: u32,
    pub schemas: usize,
    pub schema_fields: usize,
    pub schema_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionEndpointShutdownReport {
    pub before: InspectionEndpointOwnerSnapshot,
    pub schemas: crate::inspection::InspectionSchemaShutdownReport,
    pub service: crate::inspection::InspectionShutdownReport,
    pub after: InspectionEndpointOwnerSnapshot,
}

impl InspectionEndpointRuntime {
    pub fn new(
        engine: EngineId,
        schemas: InspectionSchemaRegistry,
        service: InspectionService,
    ) -> Result<Self, InspectionEndpointError> {
        if !engine.is_valid()
            || schemas.engine() != engine
            || !schemas.is_frozen()
            || service.engine() != engine
        {
            return Err(InspectionEndpointError::Malformed);
        }
        Ok(Self {
            engine,
            schemas,
            service,
            closed: false,
        })
    }

    pub const fn service(&self) -> &InspectionService {
        &self.service
    }

    pub fn owner_snapshot(&self) -> InspectionEndpointOwnerSnapshot {
        InspectionEndpointOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            undo_depth: self.service.undo_depth(),
            redo_depth: self.service.redo_depth(),
            pending_steps: self.service.pending_steps(),
            schemas: self.schemas.owner_snapshot().schemas,
            schema_fields: self.schemas.owner_snapshot().fields,
            schema_bytes: self.schemas.owner_snapshot().schema_bytes,
        }
    }

    pub fn dispatch(
        &mut self,
        world: &mut World,
        payload: &[u8],
    ) -> Result<InspectionEndpointReply, InspectionEndpointError> {
        if self.closed {
            return Err(InspectionEndpointError::Closed);
        }
        let (&tag, bytes) = payload
            .split_first()
            .ok_or(InspectionEndpointError::Malformed)?;
        let result = match tag {
            1 if bytes.len() == 16 => {
                let expected_revision = read_u64(bytes, 0)?;
                let cursor = read_u32(bytes, 8)? as usize;
                let limit = read_u32(bytes, 12)? as usize;
                if expected_revision != 0 && expected_revision != world.revision() {
                    Err(InspectionError::RevisionConflict)
                } else {
                    self.service
                        .inspect_world(world, &self.schemas, cursor, limit)
                        .and_then(encode_world_page)
                }
            }
            2 => decode_edit(self.engine, world, bytes)
                .and_then(|transaction| {
                    self.service
                        .apply_edit_at_stage(world, &self.schemas, transaction)
                })
                .map(|result| {
                    let mut output = vec![1, 2];
                    output.extend_from_slice(&result.transaction_id.to_le_bytes());
                    output.extend_from_slice(&result.world_revision.to_le_bytes());
                    output.extend_from_slice(&(result.changed_components as u32).to_le_bytes());
                    output.push(u8::from(result.history_recorded));
                    output
                }),
            3 if bytes.len() == 8 => {
                let expected = read_u64(bytes, 0)?;
                if expected != world.revision() {
                    Err(InspectionError::RevisionConflict)
                } else {
                    self.service
                        .undo(world)
                        .map(|result| encode_history(3, result))
                }
            }
            4 if bytes.len() == 8 => {
                let expected = read_u64(bytes, 0)?;
                if expected != world.revision() {
                    Err(InspectionError::RevisionConflict)
                } else {
                    self.service
                        .redo(world)
                        .map(|result| encode_history(4, result))
                }
            }
            5 if bytes.len() == 8 => {
                let _ = read_u64(bytes, 0)?;
                Ok(encode_control(5, self.service.control_revision()))
            }
            6 if bytes.len() == 12 => {
                let _ = read_u64(bytes, 0)?;
                let steps = read_u32(bytes, 8)?;
                if steps == 0 {
                    Err(InspectionError::InvalidExecutionState)
                } else {
                    Ok(encode_control(6, self.service.control_revision()))
                }
            }
            7 if bytes.len() == 8 => {
                let _ = read_u64(bytes, 0)?;
                Ok(encode_control(7, self.service.control_revision()))
            }
            _ => return Err(InspectionEndpointError::Malformed),
        };
        let control = match tag {
            5 => InspectionRequestControl::Pause {
                expected_revision: read_u64(bytes, 0)?,
            },
            6 => InspectionRequestControl::Step {
                expected_revision: read_u64(bytes, 0)?,
                steps: read_u32(bytes, 8)?,
            },
            7 => InspectionRequestControl::Resume {
                expected_revision: read_u64(bytes, 0)?,
            },
            _ => InspectionRequestControl::None,
        };
        Ok(InspectionEndpointReply {
            payload: result.unwrap_or_else(encode_error),
            control,
        })
    }

    pub fn commit_control(
        &mut self,
        control: InspectionRequestControl,
    ) -> Result<u64, InspectionError> {
        if self.closed {
            return Err(InspectionError::InvalidExecutionState);
        }
        match control {
            InspectionRequestControl::None => Ok(self.service.control_revision()),
            InspectionRequestControl::Pause { expected_revision } => {
                self.service.pause(expected_revision)
            }
            InspectionRequestControl::Step {
                expected_revision,
                steps,
            } => self.service.request_steps(expected_revision, steps),
            InspectionRequestControl::Resume { expected_revision } => {
                self.service.resume(expected_revision)
            }
        }
    }

    pub fn take_step(&mut self) -> bool {
        !self.closed && self.service.take_step()
    }

    pub fn shutdown(&mut self) -> InspectionEndpointShutdownReport {
        let before = self.owner_snapshot();
        self.closed = true;
        let schemas = self.schemas.shutdown();
        let service = self.service.shutdown();
        InspectionEndpointShutdownReport {
            before,
            schemas,
            service,
            after: self.owner_snapshot(),
        }
    }
}

fn decode_edit(
    engine: EngineId,
    world: &World,
    bytes: &[u8],
) -> Result<InspectionEditTransaction, InspectionError> {
    if bytes.len() < 20 {
        return Err(InspectionError::InvalidTransaction);
    }
    let transaction_id = read_u64(bytes, 0).map_err(|_| InspectionError::InvalidTransaction)?;
    let expected_world_revision =
        read_u64(bytes, 8).map_err(|_| InspectionError::InvalidTransaction)?;
    let count = read_u32(bytes, 16).map_err(|_| InspectionError::InvalidTransaction)? as usize;
    if count == 0 {
        return Err(InspectionError::InvalidTransaction);
    }
    let mut offset = 20;
    let mut edits = Vec::with_capacity(count);
    for _ in 0..count {
        if bytes.len().saturating_sub(offset) < 24 {
            return Err(InspectionError::InvalidTransaction);
        }
        let entity = EntityId {
            index: read_u32(bytes, offset).map_err(|_| InspectionError::InvalidTransaction)?,
            generation: read_u32(bytes, offset + 4)
                .map_err(|_| InspectionError::InvalidTransaction)?,
        };
        let component =
            read_u32(bytes, offset + 8).map_err(|_| InspectionError::InvalidTransaction)?;
        let field =
            read_u32(bytes, offset + 12).map_err(|_| InspectionError::InvalidTransaction)?;
        let value_bytes =
            read_u32(bytes, offset + 16).map_err(|_| InspectionError::InvalidTransaction)? as usize;
        if read_u32(bytes, offset + 20).map_err(|_| InspectionError::InvalidTransaction)? != 0 {
            return Err(InspectionError::InvalidTransaction);
        }
        offset = offset
            .checked_add(24)
            .ok_or(InspectionError::InvalidTransaction)?;
        let end = offset
            .checked_add(value_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or(InspectionError::InvalidTransaction)?;
        edits.push(InspectionFieldEdit {
            entity: WorldEntity {
                world: world.id(),
                entity,
            },
            component,
            field,
            value: bytes[offset..end].to_vec(),
        });
        offset = end;
    }
    if offset != bytes.len() {
        return Err(InspectionError::InvalidTransaction);
    }
    Ok(InspectionEditTransaction {
        engine,
        transaction_id,
        expected_world_revision,
        edits,
    })
}

fn encode_world_page(page: InspectionWorldPage) -> Result<Vec<u8>, InspectionError> {
    let mut output = vec![1, 1];
    output.extend_from_slice(&page.revision.to_le_bytes());
    output.extend_from_slice(&page.schema_fingerprint.to_le_bytes());
    output.extend_from_slice(&(page.next_cursor.unwrap_or(u32::MAX as usize) as u32).to_le_bytes());
    output.extend_from_slice(&(page.entities.len() as u32).to_le_bytes());
    for entity in page.entities {
        output.extend_from_slice(&entity.entity.entity.index.to_le_bytes());
        output.extend_from_slice(&entity.entity.entity.generation.to_le_bytes());
        output.extend_from_slice(&entity.stable_key.to_le_bytes());
        output.extend_from_slice(&(entity.components.len() as u32).to_le_bytes());
        for component in entity.components {
            output.extend_from_slice(&component.component.to_le_bytes());
            output.extend_from_slice(&(component.value.len() as u32).to_le_bytes());
            output.extend_from_slice(&component.value);
        }
    }
    Ok(output)
}

fn encode_history(tag: u8, result: InspectionHistoryResult) -> Vec<u8> {
    let mut output = vec![1, tag];
    output.extend_from_slice(&result.transaction_id.to_le_bytes());
    output.extend_from_slice(&result.world_revision.to_le_bytes());
    output.extend_from_slice(&(result.changed_components as u32).to_le_bytes());
    output
}

fn encode_control(tag: u8, revision: u64) -> Vec<u8> {
    let mut output = vec![1, tag];
    output.extend_from_slice(&revision.to_le_bytes());
    output
}

fn encode_error(error: InspectionError) -> Vec<u8> {
    vec![2, inspection_error_tag(&error)]
}

fn inspection_error_tag(error: &InspectionError) -> u8 {
    match error {
        InspectionError::RevisionConflict => 1,
        InspectionError::NothingToUndo => 2,
        InspectionError::NothingToRedo => 3,
        InspectionError::InvalidExecutionState => 4,
        InspectionError::RegistryNotFrozen => 5,
        InspectionError::PageCapacity
        | InspectionError::TransactionCapacity
        | InspectionError::EditByteCapacity
        | InspectionError::HistoryCapacity
        | InspectionError::StepCapacity => 6,
        _ => 255,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, InspectionEndpointError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(InspectionEndpointError::Malformed)?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, InspectionEndpointError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(InspectionEndpointError::Malformed)?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}
