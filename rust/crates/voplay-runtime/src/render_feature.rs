use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FeatureId(pub u64);

impl FeatureId {
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FeatureFactoryId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AttachmentPoint {
    BeforeDepth,
    AfterOpaque,
    BeforeTransparent,
    BeforePost,
    AfterPost,
    Overlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeatureImplementation {
    Compiled {
        factory: FeatureFactoryId,
        factory_version: u32,
    },
    DataWgsl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderBinding {
    pub group: u32,
    pub binding: u32,
    pub kind: ShaderBindingKind,
    pub min_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShaderBindingKind {
    Uniform,
    StorageRead,
    StorageReadWrite,
    Texture,
    Sampler,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderAbi {
    pub version: u32,
    pub frame_group: u32,
    pub view_group: u32,
    pub material_group: u32,
    pub object_group: u32,
    pub layout_hash: u64,
    pub bindings: Vec<ShaderBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderFeatureDescriptor {
    pub id: FeatureId,
    pub version: u32,
    pub implementation: FeatureImplementation,
    pub extractor_schema: u64,
    pub descriptor_schema: u64,
    pub shader_abi: ShaderAbi,
    pub attachment: AttachmentPoint,
    pub required_capabilities: BTreeSet<u64>,
    pub required_resources: BTreeSet<u64>,
    pub material_schema: u64,
    pub shader_hash: [u8; 32],
    pub diagnostic_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataFeatureSource {
    pub descriptor: RenderFeatureDescriptor,
    pub wgsl: String,
    pub material_defaults: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderFeatureRegistration {
    Compiled {
        descriptor: RenderFeatureDescriptor,
        closure: CompiledFeatureClosure,
    },
    Data(DataFeatureSource),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledFeatureClosure {
    pub feature: FeatureId,
    pub feature_version: u32,
    pub logic_extractor_schema: u64,
    pub logic_extractor_digest: [u8; 32],
    pub factory: FeatureFactoryId,
    pub factory_version: u32,
    pub shader_abi_version: u32,
    pub shader_layout_hash: u64,
    pub logic_artifact_digest: [u8; 32],
    pub render_artifact_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureHandshake {
    pub feature: FeatureId,
    pub version: u32,
    pub descriptor_schema: u64,
    pub extractor_schema: u64,
    pub factory: Option<(FeatureFactoryId, u32)>,
    pub shader_abi_version: u32,
    pub shader_layout_hash: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelineSpecialization {
    pub feature: FeatureId,
    pub graph_signature: u64,
    pub target_format: u32,
    pub sample_count: u32,
    pub material_variant: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealizedFeature {
    pub feature: FeatureId,
    pub revision: u64,
    pub pipeline_key: u64,
    pub attachment: AttachmentPoint,
    pub encoded_node: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeatureRegistryConfig {
    pub max_features: usize,
    pub max_factories: usize,
    pub max_wgsl_bytes: usize,
    pub max_material_bytes: usize,
    pub max_bindings: usize,
    pub max_pipelines: usize,
}

impl Default for FeatureRegistryConfig {
    fn default() -> Self {
        Self {
            max_features: 4096,
            max_factories: 4096,
            max_wgsl_bytes: 4 * 1024 * 1024,
            max_material_bytes: 4 * 1024 * 1024,
            max_bindings: 256,
            max_pipelines: 65_536,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureError {
    Closed,
    InvalidConfig,
    RegistryFrozen,
    RegistryNotFrozen,
    FeatureCapacity,
    FactoryCapacity,
    PipelineCapacity,
    InvalidDescriptor,
    DuplicateFeature,
    DuplicateFactory,
    MissingFactory,
    MissingCapability,
    ShaderAbiMismatch,
    DescriptorSchemaMismatch,
    ExtractorSchemaMismatch,
    FeatureVersionMismatch,
    InvalidWgsl,
    MaterialCapacity,
    ClosureMismatch,
    UnsupportedAttachment,
    InvalidSpecialization,
    FactoryRejected,
    RevisionExhausted,
}

pub fn encode_feature_bootstrap(features: &[Vec<u8>]) -> Result<Vec<u8>, FeatureError> {
    if features.len() > 4096 || features.iter().any(Vec::is_empty) {
        return Err(FeatureError::FeatureCapacity);
    }
    let capacity = features
        .iter()
        .try_fold(12_usize, |total, feature| {
            total.checked_add(4)?.checked_add(feature.len())
        })
        .ok_or(FeatureError::FeatureCapacity)?;
    if capacity > 64 * 1024 * 1024 {
        return Err(FeatureError::FeatureCapacity);
    }
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(b"VFRB1\0\0\0");
    bytes.extend_from_slice(&(features.len() as u32).to_le_bytes());
    for feature in features {
        bytes.extend_from_slice(&(feature.len() as u32).to_le_bytes());
        bytes.extend_from_slice(feature);
    }
    Ok(bytes)
}

pub fn decode_feature_bootstrap(bytes: &[u8]) -> Result<Vec<&[u8]>, FeatureError> {
    if bytes.len() < 12 || bytes.get(..8) != Some(b"VFRB1\0\0\0") {
        return Err(FeatureError::InvalidDescriptor);
    }
    let count = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| FeatureError::InvalidDescriptor)?,
    ) as usize;
    if count > 4096 {
        return Err(FeatureError::FeatureCapacity);
    }
    let mut offset = 12_usize;
    let mut features = Vec::with_capacity(count);
    for _ in 0..count {
        let length = u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or(FeatureError::InvalidDescriptor)?
                .try_into()
                .map_err(|_| FeatureError::InvalidDescriptor)?,
        ) as usize;
        offset = offset.checked_add(4).ok_or(FeatureError::FeatureCapacity)?;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or(FeatureError::InvalidDescriptor)?;
        if length == 0 {
            return Err(FeatureError::InvalidDescriptor);
        }
        features.push(&bytes[offset..end]);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(FeatureError::InvalidDescriptor);
    }
    Ok(features)
}

pub fn encode_routed_feature_bootstrap(
    engine: voplay_protocol::EngineId,
    features: &[Vec<u8>],
) -> Result<Vec<u8>, FeatureError> {
    if !engine.is_valid() {
        return Err(FeatureError::InvalidDescriptor);
    }
    let legacy = encode_feature_bootstrap(features)?;
    if legacy.len().saturating_add(8) > 64 * 1024 * 1024 {
        return Err(FeatureError::FeatureCapacity);
    }
    let mut bytes = Vec::with_capacity(legacy.len() + 8);
    bytes.extend_from_slice(b"VFRB2\0\0\0");
    bytes.extend_from_slice(&engine.index.to_le_bytes());
    bytes.extend_from_slice(&engine.generation.to_le_bytes());
    bytes.extend_from_slice(&legacy[8..]);
    Ok(bytes)
}

pub fn decode_routed_feature_bootstrap(
    bytes: &[u8],
) -> Result<(voplay_protocol::EngineId, Vec<&[u8]>), FeatureError> {
    if bytes.len() < 20 || bytes.get(..8) != Some(b"VFRB2\0\0\0") {
        return Err(FeatureError::InvalidDescriptor);
    }
    let engine = voplay_protocol::Handle {
        index: u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| FeatureError::InvalidDescriptor)?,
        ),
        generation: u32::from_le_bytes(
            bytes[12..16]
                .try_into()
                .map_err(|_| FeatureError::InvalidDescriptor)?,
        ),
    };
    if !engine.is_valid() {
        return Err(FeatureError::InvalidDescriptor);
    }
    let mut legacy = Vec::with_capacity(bytes.len() - 8);
    legacy.extend_from_slice(b"VFRB1\0\0\0");
    legacy.extend_from_slice(&bytes[16..]);
    let offsets = decode_feature_bootstrap(&legacy)?
        .into_iter()
        .map(|feature| {
            let start = feature.as_ptr() as usize - legacy.as_ptr() as usize;
            (start, feature.len())
        })
        .collect::<Vec<_>>();
    let features = offsets
        .into_iter()
        .map(|(start, len)| &bytes[start + 8..start + 8 + len])
        .collect();
    Ok((engine, features))
}

pub trait FeatureFactory {
    fn id(&self) -> FeatureFactoryId;
    fn version(&self) -> u32;
    fn feature(&self) -> FeatureId;
    fn realize(
        &mut self,
        descriptor: &RenderFeatureDescriptor,
        specialization: &PipelineSpecialization,
    ) -> Result<Vec<u8>, FeatureError>;
}

struct FactoryRecord {
    feature: FeatureId,
    factory: Box<dyn FeatureFactory>,
}

enum FeatureRecord {
    Compiled(RenderFeatureDescriptor),
    Data(DataFeatureSource),
}

impl FeatureRecord {
    fn descriptor(&self) -> &RenderFeatureDescriptor {
        match self {
            Self::Compiled(descriptor) => descriptor,
            Self::Data(source) => &source.descriptor,
        }
    }
}

pub struct RenderFeatureRegistry {
    config: FeatureRegistryConfig,
    capabilities: BTreeSet<u64>,
    features: BTreeMap<FeatureId, FeatureRecord>,
    factories: BTreeMap<FeatureFactoryId, FactoryRecord>,
    closures: BTreeMap<FeatureId, CompiledFeatureClosure>,
    pipelines: BTreeMap<u64, RealizedFeature>,
    frozen: bool,
    fingerprint: u64,
    revision: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderFeatureRegistryOwnerSnapshot {
    pub closed: bool,
    pub features: usize,
    pub factories: usize,
    pub compiled_closures: usize,
    pub realized_pipelines: usize,
    pub realized_pipeline_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderFeatureRegistryShutdownReport {
    pub released_features: usize,
    pub released_factories: usize,
    pub released_compiled_closures: usize,
    pub released_pipelines: usize,
    pub released_pipeline_bytes: usize,
}

impl RenderFeatureRegistry {
    pub fn new(
        config: FeatureRegistryConfig,
        capabilities: BTreeSet<u64>,
    ) -> Result<Self, FeatureError> {
        if config.max_features == 0
            || config.max_factories == 0
            || config.max_wgsl_bytes == 0
            || config.max_material_bytes == 0
            || config.max_bindings == 0
            || config.max_pipelines == 0
            || capabilities.contains(&0)
        {
            return Err(FeatureError::InvalidConfig);
        }
        Ok(Self {
            config,
            capabilities,
            features: BTreeMap::new(),
            factories: BTreeMap::new(),
            closures: BTreeMap::new(),
            pipelines: BTreeMap::new(),
            frozen: false,
            fingerprint: 0,
            revision: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> RenderFeatureRegistryOwnerSnapshot {
        RenderFeatureRegistryOwnerSnapshot {
            closed: self.closed,
            features: self.features.len(),
            factories: self.factories.len(),
            compiled_closures: self.closures.len(),
            realized_pipelines: self.pipelines.len(),
            realized_pipeline_bytes: self
                .pipelines
                .values()
                .map(|pipeline| pipeline.encoded_node.len())
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> RenderFeatureRegistryShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.features.clear();
        self.factories.clear();
        self.closures.clear();
        self.pipelines.clear();
        self.frozen = false;
        self.fingerprint = 0;
        RenderFeatureRegistryShutdownReport {
            released_features: snapshot.features,
            released_factories: snapshot.factories,
            released_compiled_closures: snapshot.compiled_closures,
            released_pipelines: snapshot.realized_pipelines,
            released_pipeline_bytes: snapshot.realized_pipeline_bytes,
        }
    }

    pub fn register_factory(
        &mut self,
        factory: Box<dyn FeatureFactory>,
    ) -> Result<(), FeatureError> {
        self.ensure_open()?;
        if self.frozen {
            return Err(FeatureError::RegistryFrozen);
        }
        if self.factories.len() >= self.config.max_factories {
            return Err(FeatureError::FactoryCapacity);
        }
        if factory.id().0 == 0 || factory.version() == 0 || !factory.feature().is_valid() {
            return Err(FeatureError::InvalidDescriptor);
        }
        let id = factory.id();
        let feature = factory.feature();
        if self
            .factories
            .insert(id, FactoryRecord { feature, factory })
            .is_some()
        {
            return Err(FeatureError::DuplicateFactory);
        }
        Ok(())
    }

    pub fn register_compiled(
        &mut self,
        descriptor: RenderFeatureDescriptor,
        closure: CompiledFeatureClosure,
    ) -> Result<(), FeatureError> {
        self.ensure_open()?;
        self.require_mutable_capacity()?;
        validate_descriptor(&descriptor, &self.config)?;
        let (factory, factory_version) = match descriptor.implementation {
            FeatureImplementation::Compiled {
                factory,
                factory_version,
            } => (factory, factory_version),
            FeatureImplementation::DataWgsl => return Err(FeatureError::InvalidDescriptor),
        };
        if closure.feature != descriptor.id
            || closure.feature_version != descriptor.version
            || closure.logic_extractor_schema != descriptor.extractor_schema
            || closure.factory != factory
            || closure.factory_version != factory_version
            || closure.shader_abi_version != descriptor.shader_abi.version
            || closure.shader_layout_hash != descriptor.shader_abi.layout_hash
            || closure.logic_extractor_digest == [0; 32]
            || closure.logic_artifact_digest == [0; 32]
            || closure.render_artifact_digest == [0; 32]
        {
            return Err(FeatureError::ClosureMismatch);
        }
        if self.features.contains_key(&descriptor.id) {
            return Err(FeatureError::DuplicateFeature);
        }
        self.closures.insert(descriptor.id, closure);
        self.features
            .insert(descriptor.id, FeatureRecord::Compiled(descriptor));
        Ok(())
    }

    pub fn register_data(&mut self, source: DataFeatureSource) -> Result<(), FeatureError> {
        self.ensure_open()?;
        self.require_mutable_capacity()?;
        validate_descriptor(&source.descriptor, &self.config)?;
        if source.descriptor.implementation != FeatureImplementation::DataWgsl
            || source.wgsl.is_empty()
            || source.wgsl.len() > self.config.max_wgsl_bytes
            || source.material_defaults.len() > self.config.max_material_bytes
            || !validate_wgsl_surface(&source.wgsl)
            || !validate_wgsl_module(&source.wgsl, Some(&source.descriptor))
        {
            return Err(FeatureError::InvalidWgsl);
        }
        if self.features.contains_key(&source.descriptor.id) {
            return Err(FeatureError::DuplicateFeature);
        }
        self.features
            .insert(source.descriptor.id, FeatureRecord::Data(source));
        Ok(())
    }

    pub fn register_wire(&mut self, bytes: &[u8]) -> Result<(), FeatureError> {
        match decode_feature_registration(bytes, self.config)? {
            RenderFeatureRegistration::Compiled {
                descriptor,
                closure,
            } => self.register_compiled(descriptor, closure),
            RenderFeatureRegistration::Data(source) => self.register_data(source),
        }
    }

    pub fn freeze(&mut self) -> Result<u64, FeatureError> {
        self.ensure_open()?;
        if self.frozen {
            return Ok(self.fingerprint);
        }
        if self.features.is_empty() {
            return Err(FeatureError::InvalidDescriptor);
        }
        for record in self.features.values() {
            let descriptor = record.descriptor();
            if !descriptor
                .required_capabilities
                .is_subset(&self.capabilities)
            {
                return Err(FeatureError::MissingCapability);
            }
            if let FeatureImplementation::Compiled {
                factory,
                factory_version,
            } = descriptor.implementation
            {
                let registered = self
                    .factories
                    .get(&factory)
                    .ok_or(FeatureError::MissingFactory)?;
                if registered.feature != descriptor.id
                    || registered.factory.version() != factory_version
                {
                    return Err(FeatureError::MissingFactory);
                }
                if !self.closures.contains_key(&descriptor.id) {
                    return Err(FeatureError::ClosureMismatch);
                }
            }
        }
        let mut hash = 0xcbf29ce484222325_u64;
        for descriptor in self.features.values().map(FeatureRecord::descriptor) {
            mix(&mut hash, &descriptor.id.0.to_le_bytes());
            mix(&mut hash, &descriptor.version.to_le_bytes());
            mix(&mut hash, &descriptor.descriptor_schema.to_le_bytes());
            mix(&mut hash, &descriptor.extractor_schema.to_le_bytes());
            mix(&mut hash, &descriptor.shader_abi.version.to_le_bytes());
            mix(&mut hash, &descriptor.shader_abi.layout_hash.to_le_bytes());
            mix(&mut hash, &descriptor.shader_hash);
            mix(&mut hash, &[descriptor.attachment as u8]);
        }
        self.fingerprint = hash.max(1);
        self.frozen = true;
        Ok(self.fingerprint)
    }

    pub fn handshake(&self, remote: &[FeatureHandshake]) -> Result<(), FeatureError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(FeatureError::RegistryNotFrozen);
        }
        if remote.len() != self.features.len() {
            return Err(FeatureError::FeatureVersionMismatch);
        }
        let remote = remote
            .iter()
            .map(|feature| (feature.feature, feature))
            .collect::<BTreeMap<_, _>>();
        for descriptor in self.features.values().map(FeatureRecord::descriptor) {
            let peer = remote
                .get(&descriptor.id)
                .ok_or(FeatureError::FeatureVersionMismatch)?;
            if peer.version != descriptor.version {
                return Err(FeatureError::FeatureVersionMismatch);
            }
            if peer.descriptor_schema != descriptor.descriptor_schema {
                return Err(FeatureError::DescriptorSchemaMismatch);
            }
            if peer.extractor_schema != descriptor.extractor_schema {
                return Err(FeatureError::ExtractorSchemaMismatch);
            }
            if peer.shader_abi_version != descriptor.shader_abi.version
                || peer.shader_layout_hash != descriptor.shader_abi.layout_hash
            {
                return Err(FeatureError::ShaderAbiMismatch);
            }
            let expected_factory = match descriptor.implementation {
                FeatureImplementation::Compiled {
                    factory,
                    factory_version,
                } => Some((factory, factory_version)),
                FeatureImplementation::DataWgsl => None,
            };
            if peer.factory != expected_factory {
                return Err(FeatureError::MissingFactory);
            }
        }
        Ok(())
    }

    pub fn realize(
        &mut self,
        specialization: PipelineSpecialization,
    ) -> Result<&RealizedFeature, FeatureError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(FeatureError::RegistryNotFrozen);
        }
        if specialization.graph_signature == 0
            || specialization.target_format == 0
            || !matches!(specialization.sample_count, 1 | 2 | 4 | 8)
        {
            return Err(FeatureError::InvalidSpecialization);
        }
        let key = pipeline_key(&specialization);
        if !self.pipelines.contains_key(&key) {
            if self.pipelines.len() >= self.config.max_pipelines {
                return Err(FeatureError::PipelineCapacity);
            }
            let record = self
                .features
                .get(&specialization.feature)
                .ok_or(FeatureError::InvalidSpecialization)?;
            let descriptor = record.descriptor().clone();
            let encoded_node = match &record {
                FeatureRecord::Compiled(_) => {
                    let factory_id = match descriptor.implementation {
                        FeatureImplementation::Compiled { factory, .. } => factory,
                        FeatureImplementation::DataWgsl => unreachable!(),
                    };
                    self.factories
                        .get_mut(&factory_id)
                        .ok_or(FeatureError::MissingFactory)?
                        .factory
                        .realize(&descriptor, &specialization)?
                }
                FeatureRecord::Data(source) => encode_data_node(source, &specialization)?,
            };
            if encoded_node.is_empty() {
                return Err(FeatureError::FactoryRejected);
            }
            self.revision = self
                .revision
                .checked_add(1)
                .ok_or(FeatureError::RevisionExhausted)?;
            self.pipelines.insert(
                key,
                RealizedFeature {
                    feature: descriptor.id,
                    revision: self.revision,
                    pipeline_key: key,
                    attachment: descriptor.attachment,
                    encoded_node,
                },
            );
        }
        Ok(self.pipelines.get(&key).unwrap())
    }

    pub fn fingerprint(&self) -> Option<u64> {
        self.frozen.then_some(self.fingerprint)
    }

    pub fn compiled_closure(&self) -> impl Iterator<Item = &CompiledFeatureClosure> {
        self.closures.values()
    }

    pub fn feature_ids(&self) -> impl Iterator<Item = FeatureId> + '_ {
        self.features.keys().copied()
    }

    pub fn handshakes(&self) -> Result<Vec<FeatureHandshake>, FeatureError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(FeatureError::RegistryNotFrozen);
        }
        Ok(self
            .features
            .values()
            .map(FeatureRecord::descriptor)
            .map(|descriptor| FeatureHandshake {
                feature: descriptor.id,
                version: descriptor.version,
                descriptor_schema: descriptor.descriptor_schema,
                extractor_schema: descriptor.extractor_schema,
                factory: match descriptor.implementation {
                    FeatureImplementation::Compiled {
                        factory,
                        factory_version,
                    } => Some((factory, factory_version)),
                    FeatureImplementation::DataWgsl => None,
                },
                shader_abi_version: descriptor.shader_abi.version,
                shader_layout_hash: descriptor.shader_abi.layout_hash,
            })
            .collect())
    }

    fn require_mutable_capacity(&self) -> Result<(), FeatureError> {
        self.ensure_open()?;
        if self.frozen {
            return Err(FeatureError::RegistryFrozen);
        }
        if self.features.len() >= self.config.max_features {
            return Err(FeatureError::FeatureCapacity);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), FeatureError> {
        if self.closed {
            Err(FeatureError::Closed)
        } else {
            Ok(())
        }
    }
}

const FEATURE_WIRE_MAGIC: &[u8; 4] = b"VRF1";
const FEATURE_WIRE_PREFIX: usize = 248;
const FEATURE_WIRE_BINDING_BYTES: usize = 24;

pub fn decode_feature_registration(
    bytes: &[u8],
    config: FeatureRegistryConfig,
) -> Result<RenderFeatureRegistration, FeatureError> {
    if bytes.len() < FEATURE_WIRE_PREFIX
        || bytes.get(..4) != Some(FEATURE_WIRE_MAGIC)
        || bytes[4] != 1
        || !matches!(bytes[5], 1 | 2)
        || !matches!(bytes[6], 1..=6)
        || bytes[7] != 0
        || bytes.get(74..76) != Some(&[0; 2])
        || bytes.get(86..88) != Some(&[0; 2])
    {
        return Err(FeatureError::InvalidDescriptor);
    }
    let capability_count = read_u16(bytes, 68)? as usize;
    let resource_count = read_u16(bytes, 70)? as usize;
    let binding_count = read_u16(bytes, 72)? as usize;
    let wgsl_len = read_u32(bytes, 76)? as usize;
    let defaults_len = read_u32(bytes, 80)? as usize;
    let label_len = read_u16(bytes, 84)? as usize;
    if binding_count > config.max_bindings
        || wgsl_len > config.max_wgsl_bytes
        || defaults_len > config.max_material_bytes
        || label_len == 0
        || label_len > 256
    {
        return Err(FeatureError::InvalidDescriptor);
    }
    let capabilities_bytes = capability_count
        .checked_mul(8)
        .ok_or(FeatureError::InvalidDescriptor)?;
    let resources_bytes = resource_count
        .checked_mul(8)
        .ok_or(FeatureError::InvalidDescriptor)?;
    let bindings_bytes = binding_count
        .checked_mul(FEATURE_WIRE_BINDING_BYTES)
        .ok_or(FeatureError::InvalidDescriptor)?;
    let expected = FEATURE_WIRE_PREFIX
        .checked_add(capabilities_bytes)
        .and_then(|size| size.checked_add(resources_bytes))
        .and_then(|size| size.checked_add(bindings_bytes))
        .and_then(|size| size.checked_add(label_len))
        .and_then(|size| size.checked_add(wgsl_len))
        .and_then(|size| size.checked_add(defaults_len))
        .ok_or(FeatureError::InvalidDescriptor)?;
    if expected != bytes.len() {
        return Err(FeatureError::InvalidDescriptor);
    }
    let mut cursor = FEATURE_WIRE_PREFIX;
    let required_capabilities = read_u64_set(bytes, &mut cursor, capability_count)?;
    let required_resources = read_u64_set(bytes, &mut cursor, resource_count)?;
    let mut bindings = Vec::with_capacity(binding_count);
    for _ in 0..binding_count {
        let kind = match bytes[cursor + 8] {
            1 => ShaderBindingKind::Uniform,
            2 => ShaderBindingKind::StorageRead,
            3 => ShaderBindingKind::StorageReadWrite,
            4 => ShaderBindingKind::Texture,
            5 => ShaderBindingKind::Sampler,
            _ => return Err(FeatureError::InvalidDescriptor),
        };
        if bytes.get(cursor + 9..cursor + 16) != Some(&[0; 7]) {
            return Err(FeatureError::InvalidDescriptor);
        }
        bindings.push(ShaderBinding {
            group: read_u32(bytes, cursor)?,
            binding: read_u32(bytes, cursor + 4)?,
            kind,
            min_size: read_u64(bytes, cursor + 16)?,
        });
        cursor += FEATURE_WIRE_BINDING_BYTES;
    }
    let label_end = cursor + label_len;
    let diagnostic_label = std::str::from_utf8(&bytes[cursor..label_end])
        .map_err(|_| FeatureError::InvalidDescriptor)?
        .to_owned();
    cursor = label_end;
    let wgsl_end = cursor + wgsl_len;
    let wgsl = std::str::from_utf8(&bytes[cursor..wgsl_end])
        .map_err(|_| FeatureError::InvalidWgsl)?
        .to_owned();
    cursor = wgsl_end;
    let material_defaults = bytes[cursor..cursor + defaults_len].to_vec();
    let factory = FeatureFactoryId(read_u64(bytes, 56)?);
    let factory_version = read_u32(bytes, 64)?;
    let implementation = if bytes[5] == 1 {
        if factory.0 == 0 || factory_version == 0 || wgsl_len != 0 || defaults_len != 0 {
            return Err(FeatureError::InvalidDescriptor);
        }
        FeatureImplementation::Compiled {
            factory,
            factory_version,
        }
    } else {
        if factory.0 != 0 || factory_version != 0 || wgsl_len == 0 {
            return Err(FeatureError::InvalidDescriptor);
        }
        FeatureImplementation::DataWgsl
    };
    let descriptor = RenderFeatureDescriptor {
        id: FeatureId(read_u64(bytes, 8)?),
        version: read_u32(bytes, 16)?,
        implementation,
        extractor_schema: read_u64(bytes, 24)?,
        descriptor_schema: read_u64(bytes, 32)?,
        shader_abi: ShaderAbi {
            version: read_u32(bytes, 20)?,
            frame_group: bytes[120] as u32,
            view_group: bytes[121] as u32,
            material_group: bytes[122] as u32,
            object_group: bytes[123] as u32,
            layout_hash: read_u64(bytes, 48)?,
            bindings,
        },
        attachment: match bytes[6] {
            1 => AttachmentPoint::BeforeDepth,
            2 => AttachmentPoint::AfterOpaque,
            3 => AttachmentPoint::BeforeTransparent,
            4 => AttachmentPoint::BeforePost,
            5 => AttachmentPoint::AfterPost,
            6 => AttachmentPoint::Overlay,
            _ => unreachable!(),
        },
        required_capabilities,
        required_resources,
        material_schema: read_u64(bytes, 40)?,
        shader_hash: bytes[88..120].try_into().unwrap(),
        diagnostic_label,
    };
    validate_descriptor(&descriptor, &config)?;
    if bytes[5] == 1 {
        let closure = CompiledFeatureClosure {
            feature: descriptor.id,
            feature_version: descriptor.version,
            logic_extractor_schema: descriptor.extractor_schema,
            logic_extractor_digest: bytes[124..156].try_into().unwrap(),
            factory,
            factory_version,
            shader_abi_version: descriptor.shader_abi.version,
            shader_layout_hash: descriptor.shader_abi.layout_hash,
            logic_artifact_digest: bytes[156..188].try_into().unwrap(),
            render_artifact_digest: bytes[188..220].try_into().unwrap(),
        };
        if bytes[220..248].iter().any(|byte| *byte != 0) {
            return Err(FeatureError::InvalidDescriptor);
        }
        Ok(RenderFeatureRegistration::Compiled {
            descriptor,
            closure,
        })
    } else {
        if bytes[124..248].iter().any(|byte| *byte != 0) {
            return Err(FeatureError::InvalidDescriptor);
        }
        Ok(RenderFeatureRegistration::Data(DataFeatureSource {
            descriptor,
            wgsl,
            material_defaults,
        }))
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, FeatureError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(FeatureError::InvalidDescriptor)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FeatureError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(FeatureError::InvalidDescriptor)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, FeatureError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(FeatureError::InvalidDescriptor)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64_set(
    bytes: &[u8],
    cursor: &mut usize,
    count: usize,
) -> Result<BTreeSet<u64>, FeatureError> {
    let mut result = BTreeSet::new();
    for _ in 0..count {
        let value = read_u64(bytes, *cursor)?;
        if value == 0 || !result.insert(value) {
            return Err(FeatureError::InvalidDescriptor);
        }
        *cursor += 8;
    }
    Ok(result)
}

fn validate_descriptor(
    descriptor: &RenderFeatureDescriptor,
    config: &FeatureRegistryConfig,
) -> Result<(), FeatureError> {
    if !descriptor.id.is_valid()
        || descriptor.version == 0
        || descriptor.extractor_schema == 0
        || descriptor.descriptor_schema == 0
        || descriptor.material_schema == 0
        || descriptor.shader_hash == [0; 32]
        || descriptor.diagnostic_label.is_empty()
        || descriptor.diagnostic_label.len() > 256
        || descriptor.required_capabilities.contains(&0)
        || descriptor.required_resources.contains(&0)
        || descriptor.shader_abi.version == 0
        || descriptor.shader_abi.layout_hash == 0
        || descriptor.shader_abi.bindings.len() > config.max_bindings
        || descriptor.shader_abi.frame_group == descriptor.shader_abi.view_group
        || descriptor.shader_abi.frame_group == descriptor.shader_abi.material_group
        || descriptor.shader_abi.frame_group == descriptor.shader_abi.object_group
        || descriptor.shader_abi.view_group == descriptor.shader_abi.material_group
        || descriptor.shader_abi.view_group == descriptor.shader_abi.object_group
        || descriptor.shader_abi.material_group == descriptor.shader_abi.object_group
    {
        return Err(FeatureError::InvalidDescriptor);
    }
    let mut slots = BTreeSet::new();
    for binding in &descriptor.shader_abi.bindings {
        if binding.min_size == 0
            || ![
                descriptor.shader_abi.frame_group,
                descriptor.shader_abi.view_group,
                descriptor.shader_abi.material_group,
                descriptor.shader_abi.object_group,
            ]
            .contains(&binding.group)
            || !slots.insert((binding.group, binding.binding))
        {
            return Err(FeatureError::InvalidDescriptor);
        }
    }
    Ok(())
}

fn validate_wgsl_surface(source: &str) -> bool {
    if source.contains("enable f16")
        || source.contains("var<storage, read_write>")
        || source.contains("@compute")
    {
        return false;
    }
    validate_wgsl_module(source, None)
}

#[cfg(feature = "render3d")]
fn validate_wgsl_module(source: &str, descriptor: Option<&RenderFeatureDescriptor>) -> bool {
    use naga::{
        valid::{Capabilities, ValidationFlags, Validator},
        AddressSpace, ShaderStage, StorageAccess, TypeInner,
    };

    let Ok(module) = naga::front::wgsl::parse_str(source) else {
        return false;
    };
    let vertex_entries = module
        .entry_points
        .iter()
        .filter(|entry| entry.stage == ShaderStage::Vertex)
        .count();
    let fragment_entries = module
        .entry_points
        .iter()
        .filter(|entry| entry.stage == ShaderStage::Fragment)
        .count();
    if Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .is_err()
        || vertex_entries != 1
        || fragment_entries != 1
        || module
            .entry_points
            .iter()
            .any(|entry| entry.stage == ShaderStage::Compute)
    {
        return false;
    }
    let Some(descriptor) = descriptor else {
        return true;
    };
    let reflected = module
        .global_variables
        .iter()
        .filter_map(|(_, variable)| {
            let binding = variable.binding.as_ref()?;
            let kind = match variable.space {
                AddressSpace::Uniform => ShaderBindingKind::Uniform,
                AddressSpace::Storage { access } if access.contains(StorageAccess::STORE) => {
                    ShaderBindingKind::StorageReadWrite
                }
                AddressSpace::Storage { .. } => ShaderBindingKind::StorageRead,
                AddressSpace::Handle => match &module.types[variable.ty].inner {
                    TypeInner::Image { .. } => ShaderBindingKind::Texture,
                    TypeInner::Sampler { .. } => ShaderBindingKind::Sampler,
                    _ => return None,
                },
                _ => return None,
            };
            Some((binding.group, binding.binding, kind))
        })
        .collect::<BTreeSet<_>>();
    let declared = descriptor
        .shader_abi
        .bindings
        .iter()
        .map(|binding| (binding.group, binding.binding, binding.kind))
        .collect::<BTreeSet<_>>();
    reflected == declared
}

#[cfg(not(feature = "render3d"))]
fn validate_wgsl_module(source: &str, _descriptor: Option<&RenderFeatureDescriptor>) -> bool {
    source.contains("@vertex") && source.contains("@fragment") && !source.contains("@compute")
}

fn encode_data_node(
    source: &DataFeatureSource,
    specialization: &PipelineSpecialization,
) -> Result<Vec<u8>, FeatureError> {
    encode_wgsl_feature_node(
        &source.descriptor,
        specialization,
        &source.wgsl,
        &source.material_defaults,
    )
}

pub fn encode_wgsl_feature_node(
    descriptor: &RenderFeatureDescriptor,
    specialization: &PipelineSpecialization,
    wgsl: &str,
    material_defaults: &[u8],
) -> Result<Vec<u8>, FeatureError> {
    if specialization.feature != descriptor.id
        || specialization.graph_signature == 0
        || specialization.target_format == 0
        || !matches!(specialization.sample_count, 1 | 2 | 4 | 8)
        || !validate_wgsl_surface(wgsl)
        || !validate_wgsl_module(wgsl, Some(descriptor))
    {
        return Err(FeatureError::InvalidSpecialization);
    }
    validate_descriptor(descriptor, &FeatureRegistryConfig::default())?;
    let total = wgsl
        .len()
        .checked_add(material_defaults.len())
        .and_then(|size| size.checked_add(descriptor.shader_abi.bindings.len().checked_mul(20)?))
        .and_then(|size| size.checked_add(68))
        .ok_or(FeatureError::MaterialCapacity)?;
    if total > 8 * 1024 * 1024 {
        return Err(FeatureError::MaterialCapacity);
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(b"VRFD2\0");
    bytes.extend_from_slice(&descriptor.id.0.to_le_bytes());
    bytes.extend_from_slice(&specialization.graph_signature.to_le_bytes());
    bytes.extend_from_slice(&specialization.target_format.to_le_bytes());
    bytes.extend_from_slice(&specialization.sample_count.to_le_bytes());
    bytes.extend_from_slice(&specialization.material_variant.to_le_bytes());
    for group in [
        descriptor.shader_abi.frame_group,
        descriptor.shader_abi.view_group,
        descriptor.shader_abi.material_group,
        descriptor.shader_abi.object_group,
    ] {
        bytes.extend_from_slice(&group.to_le_bytes());
    }
    bytes.extend_from_slice(&descriptor.shader_abi.layout_hash.to_le_bytes());
    bytes.extend_from_slice(&(descriptor.shader_abi.bindings.len() as u32).to_le_bytes());
    for binding in &descriptor.shader_abi.bindings {
        bytes.extend_from_slice(&binding.group.to_le_bytes());
        bytes.extend_from_slice(&binding.binding.to_le_bytes());
        bytes.push(match binding.kind {
            ShaderBindingKind::Uniform => 1,
            ShaderBindingKind::StorageRead => 2,
            ShaderBindingKind::StorageReadWrite => 3,
            ShaderBindingKind::Texture => 4,
            ShaderBindingKind::Sampler => 5,
        });
        bytes.extend_from_slice(&[0; 3]);
        bytes.extend_from_slice(&binding.min_size.to_le_bytes());
    }
    bytes.extend_from_slice(&(wgsl.len() as u32).to_le_bytes());
    bytes.extend_from_slice(wgsl.as_bytes());
    bytes.extend_from_slice(&(material_defaults.len() as u32).to_le_bytes());
    bytes.extend_from_slice(material_defaults);
    Ok(bytes)
}

fn pipeline_key(specialization: &PipelineSpecialization) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    mix(&mut hash, &specialization.feature.0.to_le_bytes());
    mix(&mut hash, &specialization.graph_signature.to_le_bytes());
    mix(&mut hash, &specialization.target_format.to_le_bytes());
    mix(&mut hash, &specialization.sample_count.to_le_bytes());
    mix(&mut hash, &specialization.material_variant.to_le_bytes());
    hash.max(1)
}

fn mix(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> RenderFeatureDescriptor {
        RenderFeatureDescriptor {
            id: FeatureId(1),
            version: 2,
            implementation: FeatureImplementation::DataWgsl,
            extractor_schema: 3,
            descriptor_schema: 4,
            shader_abi: ShaderAbi {
                version: 5,
                frame_group: 0,
                view_group: 1,
                material_group: 2,
                object_group: 3,
                layout_hash: 6,
                bindings: vec![ShaderBinding {
                    group: 0,
                    binding: 0,
                    kind: ShaderBindingKind::Uniform,
                    min_size: 16,
                }],
            },
            attachment: AttachmentPoint::AfterOpaque,
            required_capabilities: BTreeSet::from([7]),
            required_resources: BTreeSet::from([8]),
            material_schema: 9,
            shader_hash: [1; 32],
            diagnostic_label: "outline".to_owned(),
        }
    }

    fn source() -> DataFeatureSource {
        DataFeatureSource {
            descriptor: descriptor(),
            wgsl: r#"
struct Frame {
    value: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> frame: Frame;

@vertex
fn vs(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vertex_index) - 1);
    return vec4<f32>(x, 0.0, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return frame.value;
}
"#
            .to_owned(),
            material_defaults: vec![1, 2],
        }
    }

    #[test]
    fn data_feature_freeze_handshake_and_specialization_are_deterministic() {
        let mut registry =
            RenderFeatureRegistry::new(FeatureRegistryConfig::default(), BTreeSet::from([7]))
                .unwrap();
        registry.register_data(source()).unwrap();
        let fingerprint = registry.freeze().unwrap();
        assert_eq!(registry.freeze(), Ok(fingerprint));
        registry
            .handshake(&[FeatureHandshake {
                feature: FeatureId(1),
                version: 2,
                descriptor_schema: 4,
                extractor_schema: 3,
                factory: None,
                shader_abi_version: 5,
                shader_layout_hash: 6,
            }])
            .unwrap();
        let specialization = PipelineSpecialization {
            feature: FeatureId(1),
            graph_signature: 10,
            target_format: 11,
            sample_count: 4,
            material_variant: 12,
        };
        let first = registry.realize(specialization.clone()).unwrap().clone();
        let second = registry.realize(specialization).unwrap().clone();
        assert_eq!(first, second);
        assert_eq!(first.revision, 1);
        assert!(first.encoded_node.starts_with(b"VRFD2\0"));
    }

    #[test]
    fn unsafe_wgsl_and_missing_capability_fail_before_registry_freeze() {
        let mut registry =
            RenderFeatureRegistry::new(FeatureRegistryConfig::default(), BTreeSet::new()).unwrap();
        let mut unsafe_source = source();
        unsafe_source
            .wgsl
            .push_str("\n@compute @workgroup_size(1) fn cs() {}\n");
        assert_eq!(
            registry.register_data(unsafe_source),
            Err(FeatureError::InvalidWgsl)
        );
        registry.register_data(source()).unwrap();
        assert_eq!(registry.freeze(), Err(FeatureError::MissingCapability));
        assert_eq!(registry.fingerprint(), None);
    }
}
