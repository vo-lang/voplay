use voplay_protocol::{EngineId, Handle};
use voplay_runtime::partition_io::{
    NativeFilePartitionIoWorker, PartitionIoWork, PartitionIoWorker, PartitionIoWorkerError,
};

pub mod pipeline;
pub use pipeline::{
    ArtifactCache, ArtifactEnvelope, ArtifactPipeline, ArtifactPipelineConfig,
    ArtifactPipelineError, AssetImportContext, AssetImporter, AssetSource, CookedArtifact,
    FileAssetSource, ImportedAsset, MemoryArtifactCache, PipelineAssetWorker,
    PipelineAssetWorkerConfig, PreparedAsset, PreparedAssetDelivery, Vopack, VopackBuilder,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeAssetOwnerConfig {
    pub max_single_artifact_bytes: usize,
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

impl Default for NativeAssetOwnerConfig {
    fn default() -> Self {
        Self {
            max_single_artifact_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAssetOwnerError {
    InvalidConfig,
    WrongEngine,
    StaleProvider,
    WrongActor,
    ArtifactCapacity,
    Deadline,
    Worker(PartitionIoWorkerError),
    Closed,
}

pub struct NativeAssetOwner<W = NativeFilePartitionIoWorker> {
    engine: EngineId,
    provider_generation: Handle,
    actor: Handle,
    config: NativeAssetOwnerConfig,
    worker: W,
    closed: bool,
}

impl NativeAssetOwner<NativeFilePartitionIoWorker> {
    pub fn file(
        engine: EngineId,
        provider_generation: Handle,
        actor: Handle,
        config: NativeAssetOwnerConfig,
    ) -> Result<Self, NativeAssetOwnerError> {
        Self::new(
            engine,
            provider_generation,
            actor,
            config,
            NativeFilePartitionIoWorker,
        )
    }
}

impl<W: PartitionIoWorker> NativeAssetOwner<W> {
    pub fn new(
        engine: EngineId,
        provider_generation: Handle,
        actor: Handle,
        config: NativeAssetOwnerConfig,
        worker: W,
    ) -> Result<Self, NativeAssetOwnerError> {
        if !engine.is_valid()
            || !provider_generation.is_valid()
            || !actor.is_valid()
            || config.max_single_artifact_bytes == 0
        {
            return Err(NativeAssetOwnerError::InvalidConfig);
        }
        Ok(Self {
            engine,
            provider_generation,
            actor,
            config,
            worker,
            closed: false,
        })
    }

    pub fn execute(&mut self, work: &PartitionIoWork) -> Result<Vec<u8>, NativeAssetOwnerError> {
        self.execute_at(work, 0)
    }

    pub fn execute_at(
        &mut self,
        work: &PartitionIoWork,
        now_millis: u64,
    ) -> Result<Vec<u8>, NativeAssetOwnerError> {
        if self.closed {
            return Err(NativeAssetOwnerError::Closed);
        }
        if work.request.engine != self.engine {
            return Err(NativeAssetOwnerError::WrongEngine);
        }
        if work.provider_generation != self.provider_generation {
            return Err(NativeAssetOwnerError::StaleProvider);
        }
        if work.actor != self.actor {
            return Err(NativeAssetOwnerError::WrongActor);
        }
        if work.max_bytes > self.config.max_single_artifact_bytes {
            return Err(NativeAssetOwnerError::ArtifactCapacity);
        }
        if work.deadline_millis <= now_millis {
            return Err(NativeAssetOwnerError::Deadline);
        }
        self.worker
            .execute(work)
            .map_err(NativeAssetOwnerError::Worker)
    }

    pub fn rebind(
        &mut self,
        provider_generation: Handle,
        actor: Handle,
    ) -> Result<(), NativeAssetOwnerError> {
        if self.closed {
            return Err(NativeAssetOwnerError::Closed);
        }
        if !provider_generation.is_valid()
            || !actor.is_valid()
            || provider_generation.index != self.provider_generation.index
            || provider_generation.generation <= self.provider_generation.generation
            || actor == self.actor
        {
            return Err(NativeAssetOwnerError::StaleProvider);
        }
        self.provider_generation = provider_generation;
        self.actor = actor;
        Ok(())
    }

    pub fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use voplay_runtime::{
        partition_io::{PartitionIoRequestId, PartitionIoSource},
        world_partition::PartitionId,
    };

    struct RecordingWorker;

    impl PartitionIoWorker for RecordingWorker {
        fn execute(&mut self, work: &PartitionIoWork) -> Result<Vec<u8>, PartitionIoWorkerError> {
            Ok(vec![
                work.actor.index as u8,
                work.provider_generation.generation as u8,
            ])
        }
    }

    fn handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn work(engine: EngineId, provider_generation: Handle, actor: Handle) -> PartitionIoWork {
        PartitionIoWork {
            request: PartitionIoRequestId {
                engine,
                handle: handle(20, 1),
            },
            provider_generation,
            actor,
            partition: PartitionId(1),
            source: PartitionIoSource::File {
                path: PathBuf::from("fixture.partition"),
            },
            deadline_millis: 100,
            max_bytes: 16,
        }
    }

    #[test]
    fn owner_enforces_engine_provider_and_actor_before_worker_execution() {
        let engine = handle(1, 1);
        let provider = handle(2, 1);
        let actor = handle(3, 1);
        let mut owner = NativeAssetOwner::new(
            engine,
            provider,
            actor,
            NativeAssetOwnerConfig {
                max_single_artifact_bytes: 32,
            },
            RecordingWorker,
        )
        .unwrap();

        assert_eq!(
            owner.execute(&work(handle(9, 1), provider, actor)),
            Err(NativeAssetOwnerError::WrongEngine)
        );
        assert_eq!(
            owner.execute(&work(engine, handle(2, 2), actor)),
            Err(NativeAssetOwnerError::StaleProvider)
        );
        assert_eq!(
            owner.execute(&work(engine, provider, handle(4, 1))),
            Err(NativeAssetOwnerError::WrongActor)
        );
        assert_eq!(
            owner.execute(&work(engine, provider, actor)).unwrap(),
            [3, 1]
        );
    }

    #[test]
    fn rebind_invalidates_old_work_and_close_is_terminal() {
        let engine = handle(1, 1);
        let provider = handle(2, 1);
        let actor = handle(3, 1);
        let next_provider = handle(2, 2);
        let next_actor = handle(4, 1);
        let mut owner = NativeAssetOwner::new(
            engine,
            provider,
            actor,
            NativeAssetOwnerConfig::default(),
            RecordingWorker,
        )
        .unwrap();

        owner.rebind(next_provider, next_actor).unwrap();
        assert_eq!(
            owner.execute(&work(engine, provider, actor)),
            Err(NativeAssetOwnerError::StaleProvider)
        );
        assert_eq!(
            owner
                .execute(&work(engine, next_provider, next_actor))
                .unwrap(),
            [4, 2]
        );
        owner.close();
        assert_eq!(
            owner.execute(&work(engine, next_provider, next_actor)),
            Err(NativeAssetOwnerError::Closed)
        );
    }
}
