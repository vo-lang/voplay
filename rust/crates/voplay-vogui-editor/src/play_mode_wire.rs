use voplay_protocol::{EngineId, Handle};
use voplay_runtime::{
    asset::AssetScopeId,
    control::{ControlKind, StableControlRef},
    input::InputScopeId,
    play_mode::{
        AuthoringField, PlayModeExit, PlayModeExitReport, PlayModeId, PlayModePolicy,
        PlayModeResources, PlayModeStartReport,
    },
    world::{WorldConfig, WorldId},
};

use crate::{EditorRequest, EditorRequestKind, EditorResponse, EditorResponseOutcome};

const REQUEST_MAGIC: &[u8; 4] = b"VEP2";
const RESPONSE_MAGIC: &[u8; 4] = b"VER1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorPlayModeWireError {
    Unsupported,
    Malformed,
    Capacity,
    WrongEngine,
}

pub fn encode_play_mode_request(
    request: &EditorRequest,
    max_bytes: usize,
) -> Result<Vec<u8>, EditorPlayModeWireError> {
    if request.request_id == 0 || !request.engine.is_valid() {
        return Err(EditorPlayModeWireError::Malformed);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(REQUEST_MAGIC);
    push_u64(&mut bytes, request.request_id);
    match &request.kind {
        EditorRequestKind::StartPlayMode {
            authoring_world,
            expected_authoring_revision,
            resources,
            world_config,
            policy,
        } => {
            bytes.push(1);
            push_handle(&mut bytes, authoring_world.handle);
            push_u64(&mut bytes, *expected_authoring_revision);
            push_resources(&mut bytes, *resources);
            for value in [
                world_config.max_entities,
                world_config.max_components_per_entity,
                world_config.max_component_bytes,
                world_config.max_snapshot_bytes,
            ] {
                push_u64(
                    &mut bytes,
                    u64::try_from(value).map_err(|_| EditorPlayModeWireError::Capacity)?,
                );
            }
            push_u32(
                &mut bytes,
                u32::try_from(policy.writeback_fields.len())
                    .map_err(|_| EditorPlayModeWireError::Capacity)?,
            );
            for field in &policy.writeback_fields {
                push_u32(&mut bytes, field.component);
                push_u64(
                    &mut bytes,
                    u64::try_from(field.offset).map_err(|_| EditorPlayModeWireError::Capacity)?,
                );
                push_u64(
                    &mut bytes,
                    u64::try_from(field.width).map_err(|_| EditorPlayModeWireError::Capacity)?,
                );
            }
        }
        EditorRequestKind::ExitPlayMode { id, exit } => {
            bytes.push(2);
            push_handle(&mut bytes, id.handle);
            match exit {
                PlayModeExit::Discard => {
                    bytes.push(1);
                    push_u64(&mut bytes, 0);
                }
                PlayModeExit::ApplyAuthoring {
                    expected_authoring_revision,
                } => {
                    bytes.push(2);
                    push_u64(&mut bytes, *expected_authoring_revision);
                }
            }
        }
        _ => return Err(EditorPlayModeWireError::Unsupported),
    }
    if bytes.len() > max_bytes {
        return Err(EditorPlayModeWireError::Capacity);
    }
    Ok(bytes)
}

pub fn decode_play_mode_request(
    engine: EngineId,
    bytes: &[u8],
    max_policy_fields: usize,
) -> Result<EditorRequest, EditorPlayModeWireError> {
    if !engine.is_valid() || max_policy_fields == 0 {
        return Err(EditorPlayModeWireError::WrongEngine);
    }
    let mut reader = Reader::new(bytes, REQUEST_MAGIC)?;
    let request_id = reader.u64()?;
    if request_id == 0 {
        return Err(EditorPlayModeWireError::Malformed);
    }
    let kind = match reader.u8()? {
        1 => {
            let authoring_world = WorldId {
                engine,
                handle: reader.handle()?,
            };
            let expected_authoring_revision = reader.u64()?;
            let resources = reader.resources()?;
            let world_config = WorldConfig {
                max_entities: reader.usize()?,
                max_components_per_entity: reader.usize()?,
                max_component_bytes: reader.usize()?,
                max_snapshot_bytes: reader.usize()?,
            };
            let count = reader.u32()? as usize;
            if count > max_policy_fields {
                return Err(EditorPlayModeWireError::Capacity);
            }
            let mut writeback_fields = Vec::with_capacity(count);
            for _ in 0..count {
                writeback_fields.push(AuthoringField {
                    component: reader.u32()?,
                    offset: reader.usize()?,
                    width: reader.usize()?,
                });
            }
            EditorRequestKind::StartPlayMode {
                authoring_world,
                expected_authoring_revision,
                resources,
                world_config,
                policy: PlayModePolicy { writeback_fields },
            }
        }
        2 => {
            let id = PlayModeId {
                editor_engine: engine,
                handle: reader.handle()?,
            };
            let exit_tag = reader.u8()?;
            let expected = reader.u64()?;
            let exit = match (exit_tag, expected) {
                (1, 0) => PlayModeExit::Discard,
                (2, revision) => PlayModeExit::ApplyAuthoring {
                    expected_authoring_revision: revision,
                },
                _ => return Err(EditorPlayModeWireError::Malformed),
            };
            EditorRequestKind::ExitPlayMode { id, exit }
        }
        _ => return Err(EditorPlayModeWireError::Unsupported),
    };
    reader.finish()?;
    Ok(EditorRequest {
        request_id,
        engine,
        kind,
    })
}

pub fn encode_play_mode_response(
    response: EditorResponse,
    max_bytes: usize,
) -> Result<Vec<u8>, EditorPlayModeWireError> {
    if response.request_id == 0 || !response.engine.is_valid() {
        return Err(EditorPlayModeWireError::Malformed);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(RESPONSE_MAGIC);
    push_u64(&mut bytes, response.request_id);
    match response.outcome {
        EditorResponseOutcome::PlayModeStarted(report) => {
            bytes.push(1);
            push_handle(&mut bytes, report.id.handle);
            push_handle(&mut bytes, report.authoring_world.handle);
            push_u64(&mut bytes, report.authoring_revision);
            push_resources(&mut bytes, report.resources);
            push_u64(
                &mut bytes,
                u64::try_from(report.cloned_entities)
                    .map_err(|_| EditorPlayModeWireError::Capacity)?,
            );
            push_u64(
                &mut bytes,
                u64::try_from(report.cloned_bytes)
                    .map_err(|_| EditorPlayModeWireError::Capacity)?,
            );
        }
        EditorResponseOutcome::PlayModeExited(report) => {
            bytes.push(2);
            push_handle(&mut bytes, report.id.handle);
            push_resources(&mut bytes, report.resources);
            push_u64(
                &mut bytes,
                u64::try_from(report.changed_components)
                    .map_err(|_| EditorPlayModeWireError::Capacity)?,
            );
            match report.authoring_revision {
                Some(revision) => {
                    bytes.push(1);
                    push_u64(&mut bytes, revision);
                }
                None => {
                    bytes.push(0);
                    push_u64(&mut bytes, 0);
                }
            }
        }
        EditorResponseOutcome::RevisionConflict { actual_revision } => {
            bytes.push(3);
            push_u64(&mut bytes, actual_revision);
        }
        EditorResponseOutcome::Rejected => bytes.push(4),
        _ => return Err(EditorPlayModeWireError::Unsupported),
    }
    if bytes.len() > max_bytes {
        return Err(EditorPlayModeWireError::Capacity);
    }
    Ok(bytes)
}

pub fn decode_play_mode_response(
    engine: EngineId,
    bytes: &[u8],
) -> Result<EditorResponse, EditorPlayModeWireError> {
    if !engine.is_valid() {
        return Err(EditorPlayModeWireError::WrongEngine);
    }
    let mut reader = Reader::new(bytes, RESPONSE_MAGIC)?;
    let request_id = reader.u64()?;
    if request_id == 0 {
        return Err(EditorPlayModeWireError::Malformed);
    }
    let outcome = match reader.u8()? {
        1 => EditorResponseOutcome::PlayModeStarted(PlayModeStartReport {
            id: PlayModeId {
                editor_engine: engine,
                handle: reader.handle()?,
            },
            authoring_world: WorldId {
                engine,
                handle: reader.handle()?,
            },
            authoring_revision: reader.u64()?,
            resources: reader.resources()?,
            cloned_entities: reader.usize()?,
            cloned_bytes: reader.usize()?,
        }),
        2 => {
            let id = PlayModeId {
                editor_engine: engine,
                handle: reader.handle()?,
            };
            let resources = reader.resources()?;
            let changed_components = reader.usize()?;
            let has_revision = reader.u8()?;
            let revision = reader.u64()?;
            let authoring_revision = match (has_revision, revision) {
                (0, 0) => None,
                (1, value) => Some(value),
                _ => return Err(EditorPlayModeWireError::Malformed),
            };
            EditorResponseOutcome::PlayModeExited(PlayModeExitReport {
                id,
                resources,
                changed_components,
                authoring_revision,
            })
        }
        3 => EditorResponseOutcome::RevisionConflict {
            actual_revision: reader.u64()?,
        },
        4 => EditorResponseOutcome::Rejected,
        _ => return Err(EditorPlayModeWireError::Unsupported),
    };
    reader.finish()?;
    Ok(EditorResponse {
        request_id,
        engine,
        outcome,
    })
}

fn push_resources(bytes: &mut Vec<u8>, resources: PlayModeResources) {
    push_handle(bytes, resources.world.engine);
    push_handle(bytes, resources.world.handle);
    push_handle(bytes, resources.input_scope.handle);
    push_handle(bytes, resources.render_target.handle);
    push_handle(bytes, resources.asset_scope.handle);
}

fn push_handle(bytes: &mut Vec<u8>, handle: Handle) {
    push_u32(bytes, handle.index);
    push_u32(bytes, handle.generation);
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: &[u8; 4]) -> Result<Self, EditorPlayModeWireError> {
        if bytes.get(..4) != Some(magic) {
            return Err(EditorPlayModeWireError::Malformed);
        }
        Ok(Self { bytes, offset: 4 })
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], EditorPlayModeWireError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(EditorPlayModeWireError::Malformed)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(EditorPlayModeWireError::Malformed)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, EditorPlayModeWireError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, EditorPlayModeWireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, EditorPlayModeWireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn usize(&mut self) -> Result<usize, EditorPlayModeWireError> {
        usize::try_from(self.u64()?).map_err(|_| EditorPlayModeWireError::Capacity)
    }

    fn handle(&mut self) -> Result<Handle, EditorPlayModeWireError> {
        let handle = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        handle
            .is_valid()
            .then_some(handle)
            .ok_or(EditorPlayModeWireError::Malformed)
    }

    fn resources(&mut self) -> Result<PlayModeResources, EditorPlayModeWireError> {
        let engine = self.handle()?;
        Ok(PlayModeResources {
            world: WorldId {
                engine,
                handle: self.handle()?,
            },
            input_scope: InputScopeId {
                engine,
                handle: self.handle()?,
            },
            render_target: StableControlRef {
                engine,
                kind: ControlKind::RenderTarget,
                handle: self.handle()?,
            },
            asset_scope: AssetScopeId {
                engine,
                handle: self.handle()?,
            },
        })
    }

    fn finish(self) -> Result<(), EditorPlayModeWireError> {
        (self.offset == self.bytes.len())
            .then_some(())
            .ok_or(EditorPlayModeWireError::Malformed)
    }
}
