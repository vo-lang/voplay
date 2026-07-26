use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::Handle;

use crate::{
    scene::{
        AuthoringFieldRef, AuthoringObjectId, AuthoringObjectRef, PrefabInstancePath,
        PrefabOverride, RuntimeObjectKey, SceneInstanceId, SceneObjectSpec, SourceAssetId,
    },
    world::WorldId,
    world_partition::{PartitionChunk, PartitionId},
};

const MAGIC: [u8; 4] = *b"VPC1";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionChunkCodecConfig {
    pub max_total_bytes: usize,
    pub max_objects: usize,
    pub max_overrides: usize,
    pub max_prefab_depth: usize,
    pub max_components_per_object: usize,
    pub max_fields_per_component: usize,
    pub max_references_per_object: usize,
    pub max_value_bytes: usize,
}

impl Default for PartitionChunkCodecConfig {
    fn default() -> Self {
        Self {
            max_total_bytes: 64 * 1024 * 1024,
            max_objects: 100_000,
            max_overrides: 100_000,
            max_prefab_depth: 64,
            max_components_per_object: 256,
            max_fields_per_component: 1024,
            max_references_per_object: 1024,
            max_value_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionChunkCodecError {
    InvalidConfig,
    Malformed,
    UnsupportedVersion,
    WrongPartition,
    WrongScene,
    ObjectCapacity,
    OverrideCapacity,
    PathCapacity,
    ComponentCapacity,
    FieldCapacity,
    ReferenceCapacity,
    ValueCapacity,
    TotalByteCapacity,
}

pub fn encode_partition_chunk(
    chunk: &PartitionChunk,
    expected_scene: SceneInstanceId,
    config: PartitionChunkCodecConfig,
) -> Result<Vec<u8>, PartitionChunkCodecError> {
    validate_config(config)?;
    if !expected_scene.is_valid() {
        return Err(PartitionChunkCodecError::WrongScene);
    }
    validate_chunk(chunk, expected_scene, config)?;
    let mut objects = chunk.objects.clone();
    objects.sort_by(|first, second| first.key.cmp(&second.key));
    let mut overrides = chunk.overrides.clone();
    overrides.sort_by(|first, second| {
        (
            &first.prefab_path,
            first.authoring,
            first.component,
            first.field,
            &first.value,
        )
            .cmp(&(
                &second.prefab_path,
                second.authoring,
                second.component,
                second.field,
                &second.value,
            ))
    });
    let mut writer = Writer::new(config.max_total_bytes);
    writer.bytes(&MAGIC)?;
    writer.u32(SCHEMA_VERSION)?;
    writer.u64(chunk.id.0)?;
    writer.count(objects.len())?;
    writer.count(overrides.len())?;
    for object in &objects {
        encode_key(&mut writer, &object.key)?;
        writer.u64(object.object_type)?;
        writer.u32(object.schema_version)?;
        writer.u64(object.partition)?;
        writer.count(object.components.len())?;
        for (component, fields) in &object.components {
            writer.u32(*component)?;
            writer.count(fields.len())?;
            for (field, value) in fields {
                writer.u32(*field)?;
                writer.sized_bytes(value)?;
            }
        }
        writer.count(object.references.len())?;
        for reference in &object.references {
            writer.u32(reference.component)?;
            writer.u32(reference.field)?;
            encode_reference(&mut writer, &reference.target)?;
        }
    }
    for item in &overrides {
        encode_path(&mut writer, &item.prefab_path)?;
        encode_authoring(&mut writer, item.authoring)?;
        writer.u32(item.component)?;
        writer.u32(item.field)?;
        writer.sized_bytes(&item.value)?;
    }
    Ok(writer.finish())
}

pub fn decode_partition_chunk(
    bytes: &[u8],
    expected_partition: PartitionId,
    expected_scene: SceneInstanceId,
    config: PartitionChunkCodecConfig,
) -> Result<PartitionChunk, PartitionChunkCodecError> {
    validate_config(config)?;
    if !expected_partition.is_valid() {
        return Err(PartitionChunkCodecError::WrongPartition);
    }
    if !expected_scene.is_valid() {
        return Err(PartitionChunkCodecError::WrongScene);
    }
    if bytes.len() > config.max_total_bytes {
        return Err(PartitionChunkCodecError::TotalByteCapacity);
    }
    let mut reader = Reader::new(bytes);
    if reader.bytes(4)? != MAGIC {
        return Err(PartitionChunkCodecError::Malformed);
    }
    if reader.u32()? != SCHEMA_VERSION {
        return Err(PartitionChunkCodecError::UnsupportedVersion);
    }
    let partition = PartitionId(reader.u64()?);
    if partition != expected_partition {
        return Err(PartitionChunkCodecError::WrongPartition);
    }
    let object_count =
        reader.count(config.max_objects, PartitionChunkCodecError::ObjectCapacity)?;
    let override_count = reader.count(
        config.max_overrides,
        PartitionChunkCodecError::OverrideCapacity,
    )?;
    let mut objects = Vec::with_capacity(object_count);
    let mut keys = BTreeSet::new();
    for _ in 0..object_count {
        let key = decode_key(&mut reader, config.max_prefab_depth)?;
        if key.root_scene_instance != expected_scene {
            return Err(PartitionChunkCodecError::WrongScene);
        }
        validate_authoring(key.authoring)?;
        if !keys.insert(key.clone()) {
            return Err(PartitionChunkCodecError::Malformed);
        }
        let object_type = reader.u64()?;
        let schema_version = reader.u32()?;
        let object_partition = reader.u64()?;
        if object_type == 0 || schema_version == 0 || object_partition != partition.0 {
            return Err(PartitionChunkCodecError::WrongPartition);
        }
        let component_count = reader.count(
            config.max_components_per_object,
            PartitionChunkCodecError::ComponentCapacity,
        )?;
        let mut components = BTreeMap::new();
        for _ in 0..component_count {
            let component = reader.u32()?;
            if component == 0 || components.contains_key(&component) {
                return Err(PartitionChunkCodecError::Malformed);
            }
            let field_count = reader.count(
                config.max_fields_per_component,
                PartitionChunkCodecError::FieldCapacity,
            )?;
            let mut fields = BTreeMap::new();
            for _ in 0..field_count {
                let field = reader.u32()?;
                if field == 0 || fields.contains_key(&field) {
                    return Err(PartitionChunkCodecError::Malformed);
                }
                fields.insert(field, reader.sized_bytes(config.max_value_bytes)?);
            }
            components.insert(component, fields);
        }
        let reference_count = reader.count(
            config.max_references_per_object,
            PartitionChunkCodecError::ReferenceCapacity,
        )?;
        let mut references = Vec::with_capacity(reference_count);
        let mut reference_fields = BTreeSet::new();
        for _ in 0..reference_count {
            let component = reader.u32()?;
            let field = reader.u32()?;
            if component == 0 || field == 0 || !reference_fields.insert((component, field)) {
                return Err(PartitionChunkCodecError::Malformed);
            }
            let target = decode_reference(&mut reader, config.max_prefab_depth)?;
            validate_reference(&target, config.max_prefab_depth)?;
            references.push(AuthoringFieldRef {
                component,
                field,
                target,
            });
        }
        objects.push(SceneObjectSpec {
            key,
            object_type,
            schema_version,
            partition: object_partition,
            components,
            references,
        });
    }
    let mut overrides = Vec::with_capacity(override_count);
    let mut override_keys = BTreeSet::new();
    for _ in 0..override_count {
        let prefab_path = decode_path(&mut reader, config.max_prefab_depth)?;
        let authoring = decode_authoring(&mut reader)?;
        validate_authoring(authoring)?;
        let component = reader.u32()?;
        let field = reader.u32()?;
        let value = reader.sized_bytes(config.max_value_bytes)?;
        if component == 0
            || field == 0
            || !override_keys.insert((prefab_path.clone(), authoring, component, field))
        {
            return Err(PartitionChunkCodecError::Malformed);
        }
        overrides.push(PrefabOverride {
            prefab_path,
            authoring,
            component,
            field,
            value,
        });
    }
    if !reader.is_finished() {
        return Err(PartitionChunkCodecError::Malformed);
    }
    Ok(PartitionChunk {
        id: partition,
        objects,
        overrides,
    })
}

fn validate_config(config: PartitionChunkCodecConfig) -> Result<(), PartitionChunkCodecError> {
    if config.max_total_bytes == 0
        || config.max_objects == 0
        || config.max_overrides == 0
        || config.max_prefab_depth == 0
        || config.max_components_per_object == 0
        || config.max_fields_per_component == 0
        || config.max_references_per_object == 0
        || config.max_value_bytes == 0
    {
        return Err(PartitionChunkCodecError::InvalidConfig);
    }
    Ok(())
}

fn validate_chunk(
    chunk: &PartitionChunk,
    scene: SceneInstanceId,
    config: PartitionChunkCodecConfig,
) -> Result<(), PartitionChunkCodecError> {
    if !chunk.id.is_valid() || !scene.is_valid() {
        return Err(PartitionChunkCodecError::WrongPartition);
    }
    if chunk.objects.len() > config.max_objects {
        return Err(PartitionChunkCodecError::ObjectCapacity);
    }
    if chunk.overrides.len() > config.max_overrides {
        return Err(PartitionChunkCodecError::OverrideCapacity);
    }
    let mut keys = BTreeSet::new();
    for object in &chunk.objects {
        if object.key.root_scene_instance != scene {
            return Err(PartitionChunkCodecError::WrongScene);
        }
        if object.partition != chunk.id.0 || object.object_type == 0 || object.schema_version == 0 {
            return Err(PartitionChunkCodecError::WrongPartition);
        }
        if object.key.prefab_path.segments().len() > config.max_prefab_depth {
            return Err(PartitionChunkCodecError::PathCapacity);
        }
        if !keys.insert(object.key.clone()) {
            return Err(PartitionChunkCodecError::Malformed);
        }
        validate_authoring(object.key.authoring)?;
        if object.components.len() > config.max_components_per_object {
            return Err(PartitionChunkCodecError::ComponentCapacity);
        }
        for (component, fields) in &object.components {
            if *component == 0 {
                return Err(PartitionChunkCodecError::Malformed);
            }
            if fields.len() > config.max_fields_per_component {
                return Err(PartitionChunkCodecError::FieldCapacity);
            }
            for (field, value) in fields {
                if *field == 0 {
                    return Err(PartitionChunkCodecError::Malformed);
                }
                if value.len() > config.max_value_bytes {
                    return Err(PartitionChunkCodecError::ValueCapacity);
                }
            }
        }
        if object.references.len() > config.max_references_per_object {
            return Err(PartitionChunkCodecError::ReferenceCapacity);
        }
        let mut reference_fields = BTreeSet::new();
        for reference in &object.references {
            if reference.component == 0
                || reference.field == 0
                || !reference_fields.insert((reference.component, reference.field))
            {
                return Err(PartitionChunkCodecError::Malformed);
            }
            validate_reference(&reference.target, config.max_prefab_depth)?;
        }
    }
    let mut override_keys = BTreeSet::new();
    for item in &chunk.overrides {
        if item.prefab_path.segments().len() > config.max_prefab_depth {
            return Err(PartitionChunkCodecError::PathCapacity);
        }
        if item.value.len() > config.max_value_bytes {
            return Err(PartitionChunkCodecError::ValueCapacity);
        }
        validate_authoring(item.authoring)?;
        if item.component == 0
            || item.field == 0
            || !override_keys.insert((
                item.prefab_path.clone(),
                item.authoring,
                item.component,
                item.field,
            ))
        {
            return Err(PartitionChunkCodecError::Malformed);
        }
    }
    Ok(())
}

fn validate_authoring(authoring: AuthoringObjectId) -> Result<(), PartitionChunkCodecError> {
    if authoring.source_asset.0 == [0; 16] || authoring.local_object_id == 0 {
        return Err(PartitionChunkCodecError::Malformed);
    }
    Ok(())
}

fn validate_reference(
    reference: &AuthoringObjectRef,
    max_depth: usize,
) -> Result<(), PartitionChunkCodecError> {
    match reference {
        AuthoringObjectRef::PrefabRelative {
            relative_prefab_path,
            local_object_id,
            expected_type,
            ..
        } => {
            if relative_prefab_path.segments().len() > max_depth {
                return Err(PartitionChunkCodecError::PathCapacity);
            }
            if *local_object_id == 0 || *expected_type == 0 {
                return Err(PartitionChunkCodecError::Malformed);
            }
        }
        AuthoringObjectRef::SceneLocal {
            source_asset,
            local_object_id,
            expected_type,
            ..
        } => {
            if source_asset.0 == [0; 16] || *local_object_id == 0 || *expected_type == 0 {
                return Err(PartitionChunkCodecError::Malformed);
            }
        }
        AuthoringObjectRef::ExternalBinding {
            binding_key,
            expected_type,
            ..
        } => {
            if *binding_key == 0 || *expected_type == 0 {
                return Err(PartitionChunkCodecError::Malformed);
            }
        }
    }
    Ok(())
}

fn encode_key(writer: &mut Writer, key: &RuntimeObjectKey) -> Result<(), PartitionChunkCodecError> {
    encode_scene(writer, key.root_scene_instance)?;
    encode_path(writer, &key.prefab_path)?;
    encode_authoring(writer, key.authoring)
}

fn decode_key(
    reader: &mut Reader<'_>,
    max_depth: usize,
) -> Result<RuntimeObjectKey, PartitionChunkCodecError> {
    Ok(RuntimeObjectKey {
        root_scene_instance: decode_scene(reader)?,
        prefab_path: decode_path(reader, max_depth)?,
        authoring: decode_authoring(reader)?,
    })
}

fn encode_scene(
    writer: &mut Writer,
    scene: SceneInstanceId,
) -> Result<(), PartitionChunkCodecError> {
    encode_handle(writer, scene.world.engine)?;
    encode_handle(writer, scene.world.handle)?;
    encode_handle(writer, scene.handle)
}

fn decode_scene(reader: &mut Reader<'_>) -> Result<SceneInstanceId, PartitionChunkCodecError> {
    Ok(SceneInstanceId {
        world: WorldId {
            engine: decode_handle(reader)?,
            handle: decode_handle(reader)?,
        },
        handle: decode_handle(reader)?,
    })
}

fn encode_authoring(
    writer: &mut Writer,
    authoring: AuthoringObjectId,
) -> Result<(), PartitionChunkCodecError> {
    writer.bytes(&authoring.source_asset.0)?;
    writer.u64(authoring.local_object_id)
}

fn decode_authoring(
    reader: &mut Reader<'_>,
) -> Result<AuthoringObjectId, PartitionChunkCodecError> {
    let mut source = [0; 16];
    source.copy_from_slice(reader.bytes(16)?);
    Ok(AuthoringObjectId {
        source_asset: SourceAssetId(source),
        local_object_id: reader.u64()?,
    })
}

fn encode_path(
    writer: &mut Writer,
    path: &PrefabInstancePath,
) -> Result<(), PartitionChunkCodecError> {
    writer.count(path.segments().len())?;
    for segment in path.segments() {
        writer.u64(*segment)?;
    }
    Ok(())
}

fn decode_path(
    reader: &mut Reader<'_>,
    max_depth: usize,
) -> Result<PrefabInstancePath, PartitionChunkCodecError> {
    let count = reader.count(max_depth, PartitionChunkCodecError::PathCapacity)?;
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        segments.push(reader.u64()?);
    }
    PrefabInstancePath::new(segments).map_err(|_| PartitionChunkCodecError::Malformed)
}

fn encode_reference(
    writer: &mut Writer,
    reference: &AuthoringObjectRef,
) -> Result<(), PartitionChunkCodecError> {
    match reference {
        AuthoringObjectRef::PrefabRelative {
            relative_prefab_path,
            local_object_id,
            expected_type,
            required,
        } => {
            writer.u8(1)?;
            encode_path(writer, relative_prefab_path)?;
            writer.u64(*local_object_id)?;
            writer.u64(*expected_type)?;
            writer.bool(*required)
        }
        AuthoringObjectRef::SceneLocal {
            source_asset,
            local_object_id,
            expected_type,
            required,
        } => {
            writer.u8(2)?;
            writer.bytes(&source_asset.0)?;
            writer.u64(*local_object_id)?;
            writer.u64(*expected_type)?;
            writer.bool(*required)
        }
        AuthoringObjectRef::ExternalBinding {
            binding_key,
            expected_type,
            required,
        } => {
            writer.u8(3)?;
            writer.u64(*binding_key)?;
            writer.u64(*expected_type)?;
            writer.bool(*required)
        }
    }
}

fn decode_reference(
    reader: &mut Reader<'_>,
    max_depth: usize,
) -> Result<AuthoringObjectRef, PartitionChunkCodecError> {
    match reader.u8()? {
        1 => Ok(AuthoringObjectRef::PrefabRelative {
            relative_prefab_path: decode_path(reader, max_depth)?,
            local_object_id: reader.u64()?,
            expected_type: reader.u64()?,
            required: reader.bool()?,
        }),
        2 => {
            let mut source = [0; 16];
            source.copy_from_slice(reader.bytes(16)?);
            Ok(AuthoringObjectRef::SceneLocal {
                source_asset: SourceAssetId(source),
                local_object_id: reader.u64()?,
                expected_type: reader.u64()?,
                required: reader.bool()?,
            })
        }
        3 => Ok(AuthoringObjectRef::ExternalBinding {
            binding_key: reader.u64()?,
            expected_type: reader.u64()?,
            required: reader.bool()?,
        }),
        _ => Err(PartitionChunkCodecError::Malformed),
    }
}

fn encode_handle(writer: &mut Writer, handle: Handle) -> Result<(), PartitionChunkCodecError> {
    writer.u32(handle.index)?;
    writer.u32(handle.generation)
}

fn decode_handle(reader: &mut Reader<'_>) -> Result<Handle, PartitionChunkCodecError> {
    let handle = Handle {
        index: reader.u32()?,
        generation: reader.u32()?,
    };
    if !handle.is_valid() {
        return Err(PartitionChunkCodecError::Malformed);
    }
    Ok(handle)
}

struct Writer {
    bytes: Vec<u8>,
    max: usize,
}

impl Writer {
    fn new(max: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max,
        }
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), PartitionChunkCodecError> {
        if self.bytes.len().saturating_add(value.len()) > self.max {
            return Err(PartitionChunkCodecError::TotalByteCapacity);
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), PartitionChunkCodecError> {
        self.bytes(&[value])
    }

    fn bool(&mut self, value: bool) -> Result<(), PartitionChunkCodecError> {
        self.u8(u8::from(value))
    }

    fn u32(&mut self, value: u32) -> Result<(), PartitionChunkCodecError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), PartitionChunkCodecError> {
        self.bytes(&value.to_le_bytes())
    }

    fn count(&mut self, value: usize) -> Result<(), PartitionChunkCodecError> {
        self.u32(u32::try_from(value).map_err(|_| PartitionChunkCodecError::TotalByteCapacity)?)
    }

    fn sized_bytes(&mut self, value: &[u8]) -> Result<(), PartitionChunkCodecError> {
        self.count(value.len())?;
        self.bytes(value)
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], PartitionChunkCodecError> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(PartitionChunkCodecError::Malformed)?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, PartitionChunkCodecError> {
        Ok(self.bytes(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, PartitionChunkCodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(PartitionChunkCodecError::Malformed),
        }
    }

    fn u32(&mut self) -> Result<u32, PartitionChunkCodecError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, PartitionChunkCodecError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    fn count(
        &mut self,
        max: usize,
        error: PartitionChunkCodecError,
    ) -> Result<usize, PartitionChunkCodecError> {
        let count = self.u32()? as usize;
        if count > max {
            return Err(error);
        }
        Ok(count)
    }

    fn sized_bytes(&mut self, max: usize) -> Result<Vec<u8>, PartitionChunkCodecError> {
        let count = self.count(max, PartitionChunkCodecError::ValueCapacity)?;
        Ok(self.bytes(count)?.to_vec())
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::{
        asset::{AssetServer, AssetServerConfig},
        partition_io::{
            stable_digest, NativeFilePartitionIoWorker, PartitionIoConfig, PartitionIoRequest,
            PartitionIoScheduler, PartitionIoSource, PartitionIoWorker,
        },
        scene::{SceneConfig, SceneInstance},
        world::{World, WorldConfig},
        world_partition::{
            PartitionBounds, PartitionManifest, WorldPartition, WorldPartitionConfig,
        },
    };
    use voplay_protocol::EngineId;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn scene_id() -> SceneInstanceId {
        SceneInstanceId {
            world: WorldId {
                engine: EngineId {
                    index: 1,
                    generation: 1,
                },
                handle: handle(2),
            },
            handle: handle(3),
        }
    }

    fn object(local: u64, path: Vec<u64>) -> SceneObjectSpec {
        let source = SourceAssetId([1; 16]);
        let mut components = BTreeMap::new();
        components.insert(
            1,
            BTreeMap::from([
                (1, vec![local as u8, 2, 3]),
                (2, Vec::new()),
                (3, Vec::new()),
                (4, Vec::new()),
            ]),
        );
        SceneObjectSpec {
            key: RuntimeObjectKey {
                root_scene_instance: scene_id(),
                prefab_path: PrefabInstancePath::new(path).unwrap(),
                authoring: AuthoringObjectId {
                    source_asset: source,
                    local_object_id: local,
                },
            },
            object_type: 7,
            schema_version: 2,
            partition: 1,
            components,
            references: vec![
                AuthoringFieldRef {
                    component: 1,
                    field: 2,
                    target: AuthoringObjectRef::PrefabRelative {
                        relative_prefab_path: PrefabInstancePath::root(),
                        local_object_id: 99,
                        expected_type: 7,
                        required: false,
                    },
                },
                AuthoringFieldRef {
                    component: 1,
                    field: 3,
                    target: AuthoringObjectRef::SceneLocal {
                        source_asset: source,
                        local_object_id: 98,
                        expected_type: 7,
                        required: false,
                    },
                },
                AuthoringFieldRef {
                    component: 1,
                    field: 4,
                    target: AuthoringObjectRef::ExternalBinding {
                        binding_key: 5,
                        expected_type: 7,
                        required: false,
                    },
                },
            ],
        }
    }

    fn chunk() -> PartitionChunk {
        let first = object(1, vec![8]);
        let second = object(2, vec![7]);
        PartitionChunk {
            id: PartitionId(1),
            objects: vec![first.clone(), second],
            overrides: vec![PrefabOverride {
                prefab_path: first.key.prefab_path.clone(),
                authoring: first.key.authoring,
                component: 1,
                field: 1,
                value: vec![9, 8, 7],
            }],
        }
    }

    #[test]
    fn full_chunk_roundtrip_is_canonical_across_object_order() {
        let config = PartitionChunkCodecConfig::default();
        let chunk = chunk();
        let first = encode_partition_chunk(&chunk, scene_id(), config).unwrap();
        let mut reversed = chunk.clone();
        reversed.objects.reverse();
        reversed.overrides.reverse();
        let second = encode_partition_chunk(&reversed, scene_id(), config).unwrap();
        assert_eq!(first, second);
        let decoded = decode_partition_chunk(&first, PartitionId(1), scene_id(), config).unwrap();
        assert_eq!(decoded.id, chunk.id);
        let mut expected_objects = chunk.objects.clone();
        expected_objects.sort_by(|first, second| first.key.cmp(&second.key));
        assert_eq!(decoded.objects, expected_objects);
        assert_eq!(decoded.overrides, chunk.overrides);
    }

    #[test]
    fn malformed_owner_version_and_capacity_fail_before_chunk_creation() {
        let config = PartitionChunkCodecConfig::default();
        let bytes = encode_partition_chunk(&chunk(), scene_id(), config).unwrap();
        assert_eq!(
            decode_partition_chunk(
                &bytes[..bytes.len() - 1],
                PartitionId(1),
                scene_id(),
                config
            ),
            Err(PartitionChunkCodecError::Malformed)
        );
        assert_eq!(
            decode_partition_chunk(&bytes, PartitionId(2), scene_id(), config),
            Err(PartitionChunkCodecError::WrongPartition)
        );
        let mut foreign_scene = scene_id();
        foreign_scene.handle = handle(4);
        assert_eq!(
            decode_partition_chunk(&bytes, PartitionId(1), foreign_scene, config),
            Err(PartitionChunkCodecError::WrongScene)
        );
        let mut tiny = config;
        tiny.max_objects = 1;
        assert_eq!(
            decode_partition_chunk(&bytes, PartitionId(1), scene_id(), tiny),
            Err(PartitionChunkCodecError::ObjectCapacity)
        );
        let mut wrong_version = bytes.clone();
        wrong_version[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            decode_partition_chunk(&wrong_version, PartitionId(1), scene_id(), config),
            Err(PartitionChunkCodecError::UnsupportedVersion)
        );
    }

    #[test]
    fn native_file_verified_bytes_decode_install_and_activate_end_to_end() {
        let codec = PartitionChunkCodecConfig::default();
        let mut chunk = chunk();
        chunk.objects.truncate(1);
        chunk.overrides.truncate(1);
        let bytes = encode_partition_chunk(&chunk, scene_id(), codec).unwrap();
        let unique = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "voplay-partition-chunk-{}-{unique}.bin",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();

        let mut scheduler = PartitionIoScheduler::new(
            scene_id().world.engine,
            handle(10),
            handle(11),
            PartitionIoConfig::default(),
        )
        .unwrap();
        let request = scheduler
            .submit(PartitionIoRequest {
                partition: PartitionId(1),
                source: PartitionIoSource::File { path: path.clone() },
                priority: 1,
                deadline_millis: 100,
                max_bytes: 4096,
                expected_digest: Some(stable_digest(&bytes)),
            })
            .unwrap();
        let work = scheduler.poll_work(0).unwrap();
        let mut worker = NativeFilePartitionIoWorker;
        let completion = worker.execute(&work);
        scheduler.complete(work, completion, 1).unwrap();
        let result = scheduler.take_result(request).unwrap();

        let mut partitions = WorldPartition::new(WorldPartitionConfig::default()).unwrap();
        partitions
            .register_batch(vec![PartitionManifest {
                id: PartitionId(1),
                bounds: PartitionBounds {
                    min: [0, 0, 0],
                    max: [10, 10, 10],
                },
                priority: 0,
                dependencies: Vec::new(),
                assets: Vec::new(),
                estimated_objects: 4,
                estimated_bytes: 4096,
            }])
            .unwrap();
        let mut assets = AssetServer::new(
            scene_id().world.engine,
            handle(20),
            AssetServerConfig::default(),
        )
        .unwrap();
        partitions
            .begin_load(PartitionId(1), &mut assets, 100)
            .unwrap();
        partitions
            .install_io_result(&result, scene_id(), codec)
            .unwrap();
        let mut world = World::new(scene_id().world, WorldConfig::default()).unwrap();
        let mut scene = SceneInstance::new(scene_id(), SceneConfig::default()).unwrap();
        partitions
            .activate(
                &BTreeSet::from([PartitionId(1)]),
                &mut scene,
                &mut world,
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(world.live_entities(), 1);
        assert_eq!(partitions.source_revision(), 1);
        std::fs::remove_file(path).unwrap();
    }
}
