use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use voplay_protocol::{EngineId, Handle};
use voplay_runtime::{
    asset::{ArtifactId, AssetId, AssetRegistration, AssetWork},
    buffer_lease::{BufferLease, BufferLeaseConfig, BufferLeaseError, BufferLeaseRegistry},
};

const VOPACK_MAGIC: &[u8] = b"voplay-pack-v1\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetImportContext {
    pub canonical_locator: String,
    pub asset_type: u64,
    pub importer_id: u64,
    pub importer_version: u64,
    pub normalized_settings: Vec<u8>,
    pub target_settings: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedAsset {
    pub schema_fingerprint: [u8; 32],
    pub intermediate: Vec<u8>,
    pub metadata: Vec<u8>,
    pub dependency_artifacts: Vec<ArtifactId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CookedArtifact {
    pub asset_id: AssetId,
    pub artifact_id: ArtifactId,
    pub schema_fingerprint: [u8; 32],
    pub metadata: Vec<u8>,
    pub bytes: Arc<[u8]>,
    pub dependency_artifacts: Vec<ArtifactId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactEnvelope {
    pub artifact: CookedArtifact,
    pub cache_hit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactPipelineConfig {
    pub max_source_bytes: usize,
    pub max_intermediate_bytes: usize,
    pub max_artifact_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_dependencies: usize,
}

impl Default for ArtifactPipelineConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 512 * 1024 * 1024,
            max_intermediate_bytes: 512 * 1024 * 1024,
            max_artifact_bytes: 512 * 1024 * 1024,
            max_metadata_bytes: 4 * 1024 * 1024,
            max_dependencies: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactPipelineError {
    InvalidConfig,
    InvalidLocator,
    SourceUnavailable,
    SourceCapacity,
    ImportFailed,
    ImportCapacity,
    CookFailed,
    ArtifactCapacity,
    MetadataCapacity,
    DependencyCapacity,
    DependencyOrder,
    StaleSourceRevision,
    CacheCapacity,
    PackMalformed,
    PackCapacity,
    ArithmeticOverflow,
}

pub trait AssetSource {
    fn fetch(
        &mut self,
        canonical_locator: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArtifactPipelineError>;
}

pub trait AssetImporter {
    fn import(
        &mut self,
        context: &AssetImportContext,
        source: &[u8],
    ) -> Result<ImportedAsset, ArtifactPipelineError>;

    fn cook(
        &mut self,
        context: &AssetImportContext,
        imported: &ImportedAsset,
    ) -> Result<Vec<u8>, ArtifactPipelineError>;
}

pub trait ArtifactCache {
    fn get(&mut self, artifact: ArtifactId) -> Option<CookedArtifact>;
    fn put(&mut self, artifact: CookedArtifact) -> Result<(), ArtifactPipelineError>;
}

pub struct ArtifactPipeline<S, I, C> {
    config: ArtifactPipelineConfig,
    source: S,
    importer: I,
    cache: C,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipelineAssetWorkerConfig {
    pub max_prepared_assets: usize,
    pub leases: BufferLeaseConfig,
}

impl Default for PipelineAssetWorkerConfig {
    fn default() -> Self {
        Self {
            max_prepared_assets: 100_000,
            leases: BufferLeaseConfig {
                max_leases: 4096,
                max_total_bytes: 512 * 1024 * 1024,
                max_chunk_bytes: 8 * 1024 * 1024,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedAsset {
    pub registration: AssetRegistration,
    pub artifact: CookedArtifact,
}

#[derive(Clone, Debug)]
pub struct PreparedAssetDelivery {
    pub work: AssetWork,
    pub lease: BufferLease,
    pub schema_fingerprint: [u8; 32],
    pub metadata: Vec<u8>,
}

pub struct PipelineAssetWorker<S, I, C> {
    pipeline: ArtifactPipeline<S, I, C>,
    prepared: BTreeMap<AssetId, PreparedAsset>,
    leases: BufferLeaseRegistry,
    max_prepared_assets: usize,
}

impl<S: AssetSource, I: AssetImporter, C: ArtifactCache> PipelineAssetWorker<S, I, C> {
    pub fn new(
        engine: EngineId,
        provider_generation: Handle,
        config: PipelineAssetWorkerConfig,
        pipeline: ArtifactPipeline<S, I, C>,
    ) -> Result<Self, ArtifactPipelineError> {
        if config.max_prepared_assets == 0 {
            return Err(ArtifactPipelineError::InvalidConfig);
        }
        let leases = BufferLeaseRegistry::new(engine, provider_generation, config.leases)
            .map_err(map_lease_error)?;
        Ok(Self {
            pipeline,
            prepared: BTreeMap::new(),
            leases,
            max_prepared_assets: config.max_prepared_assets,
        })
    }

    pub fn prepare(
        &mut self,
        context: &AssetImportContext,
        source_revision: u64,
        dependencies: Vec<AssetId>,
    ) -> Result<PreparedAsset, ArtifactPipelineError> {
        if source_revision == 0 || dependencies.len() > self.pipeline.config.max_dependencies {
            return Err(ArtifactPipelineError::DependencyCapacity);
        }
        let envelope = self.pipeline.build(context)?;
        if self
            .prepared
            .get(&envelope.artifact.asset_id)
            .is_some_and(|current| current.registration.source_revision >= source_revision)
        {
            return Err(ArtifactPipelineError::StaleSourceRevision);
        }
        let registration = AssetRegistration {
            asset_id: envelope.artifact.asset_id,
            asset_type: context.asset_type,
            source_revision,
            artifact_id: envelope.artifact.artifact_id,
            dependencies,
        };
        let prepared = PreparedAsset {
            registration,
            artifact: envelope.artifact,
        };
        if !self.prepared.contains_key(&prepared.registration.asset_id)
            && self.prepared.len() == self.max_prepared_assets
        {
            return Err(ArtifactPipelineError::CacheCapacity);
        }
        self.prepared
            .insert(prepared.registration.asset_id, prepared.clone());
        Ok(prepared)
    }

    pub fn registration(&self, asset_id: AssetId) -> Option<&AssetRegistration> {
        self.prepared
            .get(&asset_id)
            .map(|prepared| &prepared.registration)
    }

    pub fn deliver(
        &mut self,
        work: AssetWork,
        consumer: Handle,
        deadline_millis: u64,
    ) -> Result<PreparedAssetDelivery, ArtifactPipelineError> {
        let prepared = self
            .prepared
            .get(&work.asset_id)
            .ok_or(ArtifactPipelineError::SourceUnavailable)?;
        if work.source_revision != prepared.registration.source_revision
            || work.artifact_id != prepared.registration.artifact_id
            || work.endpoint_generation != self.leases.provider_generation()
        {
            return Err(ArtifactPipelineError::DependencyOrder);
        }
        let lease = self
            .leases
            .issue(
                consumer,
                prepared.artifact.artifact_id,
                prepared.artifact.bytes.to_vec(),
                deadline_millis,
            )
            .map_err(map_lease_error)?;
        Ok(PreparedAssetDelivery {
            work,
            lease,
            schema_fingerprint: prepared.artifact.schema_fingerprint,
            metadata: prepared.artifact.metadata.clone(),
        })
    }

    pub fn leases(&self) -> &BufferLeaseRegistry {
        &self.leases
    }

    pub fn leases_mut(&mut self) -> &mut BufferLeaseRegistry {
        &mut self.leases
    }

    pub fn expire_leases(&mut self, now_millis: u64) -> Result<usize, ArtifactPipelineError> {
        self.leases.expire(now_millis).map_err(map_lease_error)
    }

    pub fn restart_provider(&mut self) -> Result<usize, ArtifactPipelineError> {
        self.leases.restart_provider().map_err(map_lease_error)
    }

    pub fn release_prepared(&mut self, asset_id: AssetId) -> Option<PreparedAsset> {
        self.prepared.remove(&asset_id)
    }
}

impl<S: AssetSource, I: AssetImporter, C: ArtifactCache> ArtifactPipeline<S, I, C> {
    pub fn new(
        config: ArtifactPipelineConfig,
        source: S,
        importer: I,
        cache: C,
    ) -> Result<Self, ArtifactPipelineError> {
        if config.max_source_bytes == 0
            || config.max_intermediate_bytes == 0
            || config.max_artifact_bytes == 0
            || config.max_metadata_bytes == 0
            || config.max_dependencies == 0
        {
            return Err(ArtifactPipelineError::InvalidConfig);
        }
        Ok(Self {
            config,
            source,
            importer,
            cache,
        })
    }

    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    pub fn importer_mut(&mut self) -> &mut I {
        &mut self.importer
    }

    pub fn cache_mut(&mut self) -> &mut C {
        &mut self.cache
    }

    pub fn build(
        &mut self,
        context: &AssetImportContext,
    ) -> Result<ArtifactEnvelope, ArtifactPipelineError> {
        validate_context(context)?;
        let asset_id = derive_asset_id(context);
        let source = self
            .source
            .fetch(&context.canonical_locator, self.config.max_source_bytes)?;
        if source.len() > self.config.max_source_bytes {
            return Err(ArtifactPipelineError::SourceCapacity);
        }
        let imported = self
            .importer
            .import(context, &source)
            .map_err(|_| ArtifactPipelineError::ImportFailed)?;
        self.validate_imported(&imported)?;
        let cooked = self
            .importer
            .cook(context, &imported)
            .map_err(|_| ArtifactPipelineError::CookFailed)?;
        if cooked.len() > self.config.max_artifact_bytes {
            return Err(ArtifactPipelineError::ArtifactCapacity);
        }
        let artifact_id = derive_artifact_id(context, &imported, &cooked);
        if let Some(artifact) = self.cache.get(artifact_id) {
            return Ok(ArtifactEnvelope {
                artifact,
                cache_hit: true,
            });
        }
        let artifact = CookedArtifact {
            asset_id,
            artifact_id,
            schema_fingerprint: imported.schema_fingerprint,
            metadata: imported.metadata,
            bytes: Arc::from(cooked),
            dependency_artifacts: imported.dependency_artifacts,
        };
        self.cache.put(artifact.clone())?;
        Ok(ArtifactEnvelope {
            artifact,
            cache_hit: false,
        })
    }

    fn validate_imported(&self, imported: &ImportedAsset) -> Result<(), ArtifactPipelineError> {
        if imported.intermediate.len() > self.config.max_intermediate_bytes {
            return Err(ArtifactPipelineError::ImportCapacity);
        }
        if imported.metadata.len() > self.config.max_metadata_bytes {
            return Err(ArtifactPipelineError::MetadataCapacity);
        }
        if imported.dependency_artifacts.len() > self.config.max_dependencies {
            return Err(ArtifactPipelineError::DependencyCapacity);
        }
        if imported
            .dependency_artifacts
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(ArtifactPipelineError::DependencyOrder);
        }
        Ok(())
    }
}

pub struct FileAssetSource {
    root: PathBuf,
}

impl FileAssetSource {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ArtifactPipelineError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(ArtifactPipelineError::InvalidLocator);
        }
        Ok(Self { root })
    }
}

impl AssetSource for FileAssetSource {
    fn fetch(
        &mut self,
        canonical_locator: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArtifactPipelineError> {
        let relative = Path::new(canonical_locator);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ArtifactPipelineError::InvalidLocator);
        }
        let file = File::open(self.root.join(relative))
            .map_err(|_| ArtifactPipelineError::SourceUnavailable)?;
        let length = file
            .metadata()
            .map_err(|_| ArtifactPipelineError::SourceUnavailable)?
            .len();
        if length > max_bytes as u64 {
            return Err(ArtifactPipelineError::SourceCapacity);
        }
        let mut output = Vec::with_capacity(length as usize);
        file.take(
            max_bytes
                .checked_add(1)
                .ok_or(ArtifactPipelineError::SourceCapacity)? as u64,
        )
        .read_to_end(&mut output)
        .map_err(|_| ArtifactPipelineError::SourceUnavailable)?;
        if output.len() > max_bytes {
            return Err(ArtifactPipelineError::SourceCapacity);
        }
        Ok(output)
    }
}

pub struct MemoryArtifactCache {
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    entries: BTreeMap<ArtifactId, CookedArtifact>,
    lru: VecDeque<ArtifactId>,
}

impl MemoryArtifactCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> Result<Self, ArtifactPipelineError> {
        if max_entries == 0 || max_bytes == 0 {
            return Err(ArtifactPipelineError::InvalidConfig);
        }
        Ok(Self {
            max_entries,
            max_bytes,
            bytes: 0,
            entries: BTreeMap::new(),
            lru: VecDeque::new(),
        })
    }

    pub const fn resident_bytes(&self) -> usize {
        self.bytes
    }

    fn touch(&mut self, id: ArtifactId) {
        self.lru.retain(|entry| *entry != id);
        self.lru.push_back(id);
    }
}

impl ArtifactCache for MemoryArtifactCache {
    fn get(&mut self, artifact: ArtifactId) -> Option<CookedArtifact> {
        let value = self.entries.get(&artifact).cloned()?;
        self.touch(artifact);
        Some(value)
    }

    fn put(&mut self, artifact: CookedArtifact) -> Result<(), ArtifactPipelineError> {
        let size = artifact
            .bytes
            .len()
            .checked_add(artifact.metadata.len())
            .ok_or(ArtifactPipelineError::ArithmeticOverflow)?;
        if size > self.max_bytes {
            return Err(ArtifactPipelineError::CacheCapacity);
        }
        if let Some(previous) = self.entries.remove(&artifact.artifact_id) {
            self.bytes -= previous.bytes.len() + previous.metadata.len();
            self.lru.retain(|entry| *entry != artifact.artifact_id);
        }
        while self.entries.len() >= self.max_entries
            || self
                .bytes
                .checked_add(size)
                .is_none_or(|bytes| bytes > self.max_bytes)
        {
            let oldest = self
                .lru
                .pop_front()
                .ok_or(ArtifactPipelineError::CacheCapacity)?;
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.bytes.len() + evicted.metadata.len();
            }
        }
        self.bytes += size;
        self.touch(artifact.artifact_id);
        self.entries.insert(artifact.artifact_id, artifact);
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct VopackEntry {
    offset: u64,
    len: usize,
}

pub struct Vopack {
    bytes: Arc<[u8]>,
    entries: BTreeMap<ArtifactId, VopackEntry>,
}

impl Vopack {
    pub fn parse(bytes: Arc<[u8]>, max_entries: usize) -> Result<Self, ArtifactPipelineError> {
        if !bytes.starts_with(VOPACK_MAGIC) || bytes.len() < VOPACK_MAGIC.len() + 4 {
            return Err(ArtifactPipelineError::PackMalformed);
        }
        let mut offset = VOPACK_MAGIC.len();
        let count = read_u32(&bytes, &mut offset)? as usize;
        if count > max_entries {
            return Err(ArtifactPipelineError::PackCapacity);
        }
        let index_bytes = count
            .checked_mul(28)
            .ok_or(ArtifactPipelineError::PackCapacity)?;
        let data_start = offset
            .checked_add(index_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or(ArtifactPipelineError::PackMalformed)?;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let mut id = [0_u8; 16];
            id.copy_from_slice(
                bytes
                    .get(offset..offset + 16)
                    .ok_or(ArtifactPipelineError::PackMalformed)?,
            );
            offset += 16;
            let entry_offset = read_u64(&bytes, &mut offset)?;
            let len = read_u32(&bytes, &mut offset)? as usize;
            let end = (entry_offset as usize)
                .checked_add(len)
                .filter(|end| *end <= bytes.len())
                .ok_or(ArtifactPipelineError::PackMalformed)?;
            if entry_offset < data_start as u64 || end < data_start {
                return Err(ArtifactPipelineError::PackMalformed);
            }
            if entries
                .insert(
                    ArtifactId(id),
                    VopackEntry {
                        offset: entry_offset,
                        len,
                    },
                )
                .is_some()
            {
                return Err(ArtifactPipelineError::PackMalformed);
            }
        }
        Ok(Self { bytes, entries })
    }

    pub fn artifact(&self, id: ArtifactId) -> Option<&[u8]> {
        let entry = self.entries.get(&id)?;
        self.bytes
            .get(entry.offset as usize..entry.offset as usize + entry.len)
    }
}

pub struct VopackBuilder {
    max_entries: usize,
    max_bytes: usize,
    entries: BTreeMap<ArtifactId, Arc<[u8]>>,
}

impl VopackBuilder {
    pub fn new(max_entries: usize, max_bytes: usize) -> Result<Self, ArtifactPipelineError> {
        if max_entries == 0 || max_bytes == 0 {
            return Err(ArtifactPipelineError::InvalidConfig);
        }
        Ok(Self {
            max_entries,
            max_bytes,
            entries: BTreeMap::new(),
        })
    }

    pub fn insert(&mut self, artifact: &CookedArtifact) -> Result<(), ArtifactPipelineError> {
        if !self.entries.contains_key(&artifact.artifact_id)
            && self.entries.len() == self.max_entries
        {
            return Err(ArtifactPipelineError::PackCapacity);
        }
        self.entries
            .insert(artifact.artifact_id, Arc::clone(&artifact.bytes));
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<u8>, ArtifactPipelineError> {
        let index_bytes = self
            .entries
            .len()
            .checked_mul(28)
            .ok_or(ArtifactPipelineError::PackCapacity)?;
        let data_start = VOPACK_MAGIC
            .len()
            .checked_add(4)
            .and_then(|bytes| bytes.checked_add(index_bytes))
            .ok_or(ArtifactPipelineError::PackCapacity)?;
        let total = self.entries.values().try_fold(data_start, |total, bytes| {
            total
                .checked_add(bytes.len())
                .ok_or(ArtifactPipelineError::PackCapacity)
        })?;
        if total > self.max_bytes || total > u32::MAX as usize {
            return Err(ArtifactPipelineError::PackCapacity);
        }
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(VOPACK_MAGIC);
        output.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut data_offset = data_start as u64;
        for (id, bytes) in &self.entries {
            output.extend_from_slice(&id.0);
            output.extend_from_slice(&data_offset.to_le_bytes());
            output.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            data_offset += bytes.len() as u64;
        }
        for bytes in self.entries.values() {
            output.extend_from_slice(bytes);
        }
        Ok(output)
    }
}

fn validate_context(context: &AssetImportContext) -> Result<(), ArtifactPipelineError> {
    if context.canonical_locator.is_empty()
        || context.asset_type == 0
        || context.importer_id == 0
        || context.importer_version == 0
    {
        return Err(ArtifactPipelineError::InvalidLocator);
    }
    Ok(())
}

fn derive_asset_id(context: &AssetImportContext) -> AssetId {
    let mut digest = StableDigest128::new();
    digest.bytes(context.canonical_locator.as_bytes());
    digest.u64(context.asset_type);
    digest.u64(context.importer_id);
    digest.u64(context.importer_version);
    digest.bytes(&context.normalized_settings);
    AssetId(digest.finish())
}

fn derive_artifact_id(
    context: &AssetImportContext,
    imported: &ImportedAsset,
    cooked: &[u8],
) -> ArtifactId {
    let mut digest = StableDigest128::new();
    digest.bytes(cooked);
    digest.u64(context.importer_id);
    digest.u64(context.importer_version);
    digest.bytes(&context.target_settings);
    for dependency in &imported.dependency_artifacts {
        digest.bytes(&dependency.0);
    }
    ArtifactId(digest.finish())
}

struct StableDigest128 {
    lanes: [u64; 2],
}

impl StableDigest128 {
    fn new() -> Self {
        Self {
            lanes: [0xcbf2_9ce4_8422_2325, 0x8422_2325_cbf2_9ce4],
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for (index, byte) in bytes.iter().copied().enumerate() {
            let lane = index & 1;
            self.lanes[lane] ^= u64::from(byte);
            self.lanes[lane] = self.lanes[lane].wrapping_mul(0x0000_0100_0000_01b3);
            self.lanes[lane] ^= self.lanes[lane] >> 29;
        }
        for lane in &mut self.lanes {
            *lane ^= bytes.len() as u64;
            *lane = lane.rotate_left(17);
        }
    }

    fn finish(self) -> [u8; 16] {
        let mut output = [0_u8; 16];
        output[..8].copy_from_slice(&self.lanes[0].to_le_bytes());
        output[8..].copy_from_slice(&self.lanes[1].to_le_bytes());
        output
    }
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, ArtifactPipelineError> {
    let value = u32::from_le_bytes(
        bytes
            .get(*offset..*offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ArtifactPipelineError::PackMalformed)?,
    );
    *offset += 4;
    Ok(value)
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, ArtifactPipelineError> {
    let value = u64::from_le_bytes(
        bytes
            .get(*offset..*offset + 8)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ArtifactPipelineError::PackMalformed)?,
    );
    *offset += 8;
    Ok(value)
}

fn map_lease_error(error: BufferLeaseError) -> ArtifactPipelineError {
    match error {
        BufferLeaseError::Capacity | BufferLeaseError::ChunkCapacity => {
            ArtifactPipelineError::CacheCapacity
        }
        BufferLeaseError::OutOfBounds | BufferLeaseError::DigestMismatch => {
            ArtifactPipelineError::ArtifactCapacity
        }
        _ => ArtifactPipelineError::InvalidConfig,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_runtime::asset::{AssetRef, AssetTicket};

    #[derive(Default)]
    struct MemorySource {
        fetches: usize,
    }

    impl AssetSource for MemorySource {
        fn fetch(
            &mut self,
            canonical_locator: &str,
            max_bytes: usize,
        ) -> Result<Vec<u8>, ArtifactPipelineError> {
            self.fetches += 1;
            let bytes = format!("source:{canonical_locator}").into_bytes();
            (bytes.len() <= max_bytes)
                .then_some(bytes)
                .ok_or(ArtifactPipelineError::SourceCapacity)
        }
    }

    #[derive(Default)]
    struct CopyImporter {
        imports: usize,
        cooks: usize,
    }

    impl AssetImporter for CopyImporter {
        fn import(
            &mut self,
            _context: &AssetImportContext,
            source: &[u8],
        ) -> Result<ImportedAsset, ArtifactPipelineError> {
            self.imports += 1;
            Ok(ImportedAsset {
                schema_fingerprint: [7; 32],
                intermediate: source.to_vec(),
                metadata: b"fixture".to_vec(),
                dependency_artifacts: Vec::new(),
            })
        }

        fn cook(
            &mut self,
            _context: &AssetImportContext,
            imported: &ImportedAsset,
        ) -> Result<Vec<u8>, ArtifactPipelineError> {
            self.cooks += 1;
            let mut cooked = b"cooked:".to_vec();
            cooked.extend_from_slice(&imported.intermediate);
            Ok(cooked)
        }
    }

    fn handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn context() -> AssetImportContext {
        AssetImportContext {
            canonical_locator: "textures/hero.rgba".into(),
            asset_type: 1,
            importer_id: 2,
            importer_version: 3,
            normalized_settings: b"linear".to_vec(),
            target_settings: b"portable".to_vec(),
        }
    }

    fn pipeline() -> ArtifactPipeline<MemorySource, CopyImporter, MemoryArtifactCache> {
        ArtifactPipeline::new(
            ArtifactPipelineConfig::default(),
            MemorySource::default(),
            CopyImporter::default(),
            MemoryArtifactCache::new(8, 4096).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn deterministic_pipeline_cache_and_pack_round_trip_preserve_artifact_identity() {
        let mut pipeline = pipeline();
        let first = pipeline.build(&context()).unwrap();
        let second = pipeline.build(&context()).unwrap();

        assert!(!first.cache_hit);
        assert!(second.cache_hit);
        assert_eq!(first.artifact, second.artifact);
        assert_eq!(pipeline.source_mut().fetches, 2);
        assert_eq!(pipeline.importer_mut().imports, 2);
        assert_eq!(pipeline.importer_mut().cooks, 2);

        let mut builder = VopackBuilder::new(2, 4096).unwrap();
        builder.insert(&first.artifact).unwrap();
        let bytes = builder.finish().unwrap();
        let pack = Vopack::parse(Arc::from(bytes), 2).unwrap();
        assert_eq!(
            pack.artifact(first.artifact.artifact_id),
            Some(first.artifact.bytes.as_ref())
        );
    }

    #[test]
    fn prepared_delivery_rejects_stale_work_and_provider_restart_revokes_lease() {
        let engine = handle(1, 1);
        let provider = handle(2, 4);
        let consumer = handle(3, 1);
        let mut worker = PipelineAssetWorker::new(
            engine,
            provider,
            PipelineAssetWorkerConfig::default(),
            pipeline(),
        )
        .unwrap();
        let prepared = worker.prepare(&context(), 9, Vec::new()).unwrap();
        let work = AssetWork {
            ticket: AssetTicket {
                engine,
                handle: handle(10, 1),
            },
            asset_ref: AssetRef {
                engine,
                handle: handle(11, 1),
            },
            asset_id: prepared.registration.asset_id,
            source_revision: prepared.registration.source_revision,
            artifact_id: prepared.registration.artifact_id,
            endpoint_generation: provider,
        };
        let mut stale = work.clone();
        stale.source_revision -= 1;
        assert!(matches!(
            worker.deliver(stale, consumer, 100),
            Err(ArtifactPipelineError::DependencyOrder)
        ));

        let delivery = worker.deliver(work, consumer, 100).unwrap();
        let chunk_len = delivery.lease.len.min(8);
        assert_eq!(
            worker
                .leases()
                .read_chunk(delivery.lease, consumer, 1, 0, chunk_len)
                .unwrap(),
            prepared.artifact.bytes[..chunk_len]
        );
        assert_eq!(worker.restart_provider().unwrap(), 1);
        assert_eq!(
            worker.leases().open_read(delivery.lease, consumer, 1),
            Err(BufferLeaseError::StaleProvider)
        );
        assert_eq!(
            worker.registration(prepared.registration.asset_id),
            Some(&prepared.registration)
        );
    }
}
