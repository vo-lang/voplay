use std::cell::Cell;

use voplay_protocol::{EngineId, Handle};

use crate::asset::ArtifactId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferLeaseConfig {
    pub max_leases: usize,
    pub max_total_bytes: usize,
    pub max_chunk_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferLease {
    pub engine: EngineId,
    pub provider_generation: Handle,
    pub consumer: Handle,
    pub handle: Handle,
    pub artifact_id: ArtifactId,
    pub len: usize,
    pub deadline_millis: u64,
}

pub struct BufferSpan<'a>(&'a [u8]);

impl BufferSpan<'_> {
    pub fn bytes(&self) -> &[u8] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferLeaseError {
    Closed,
    InvalidConfig,
    InvalidOwner,
    Capacity,
    WrongEngine,
    StaleProvider,
    WrongConsumer,
    InvalidLease,
    Expired,
    ChunkCapacity,
    OutOfBounds,
    DigestMismatch,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BufferLeaseMetrics {
    pub live: usize,
    pub peak_live: usize,
    pub total_bytes: usize,
    pub peak_total_bytes: usize,
    pub issued: u64,
    pub released: u64,
    pub expired: u64,
    pub provider_restarts: u64,
    pub stale_rejections: u64,
}

struct Record {
    generation: u32,
    consumer: Handle,
    artifact_id: ArtifactId,
    bytes: Vec<u8>,
    deadline_millis: u64,
}

pub struct BufferLeaseRegistry {
    engine: EngineId,
    provider_generation: Handle,
    config: BufferLeaseConfig,
    slots: Vec<Option<Record>>,
    generations: Vec<u32>,
    free: Vec<u32>,
    live: usize,
    total_bytes: usize,
    metrics: Cell<BufferLeaseMetrics>,
    closed: bool,
}

impl BufferLeaseRegistry {
    pub fn new(
        engine: EngineId,
        provider_generation: Handle,
        config: BufferLeaseConfig,
    ) -> Result<Self, BufferLeaseError> {
        if !engine.is_valid()
            || !provider_generation.is_valid()
            || config.max_leases == 0
            || config.max_leases > u32::MAX as usize
            || config.max_total_bytes == 0
            || config.max_chunk_bytes == 0
        {
            return Err(BufferLeaseError::InvalidConfig);
        }
        Ok(Self {
            engine,
            provider_generation,
            config,
            slots: Vec::new(),
            generations: Vec::new(),
            free: Vec::new(),
            live: 0,
            total_bytes: 0,
            metrics: Cell::new(BufferLeaseMetrics::default()),
            closed: false,
        })
    }

    pub const fn provider_generation(&self) -> Handle {
        self.provider_generation
    }

    pub fn issue(
        &mut self,
        consumer: Handle,
        artifact_id: ArtifactId,
        bytes: Vec<u8>,
        deadline_millis: u64,
    ) -> Result<BufferLease, BufferLeaseError> {
        self.ensure_open()?;
        if !consumer.is_valid() || artifact_id.0 == [0; 16] {
            return Err(BufferLeaseError::InvalidOwner);
        }
        if self.live == self.config.max_leases {
            return Err(BufferLeaseError::Capacity);
        }
        let total = self
            .total_bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= self.config.max_total_bytes)
            .ok_or(BufferLeaseError::Capacity)?;
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(None);
            self.generations.push(1);
            index
        };
        let generation = self.generations[index as usize];
        let len = bytes.len();
        self.slots[index as usize] = Some(Record {
            generation,
            consumer,
            artifact_id,
            bytes,
            deadline_millis,
        });
        self.live += 1;
        self.total_bytes = total;
        let mut metrics = self.metrics.get();
        metrics.live = self.live;
        metrics.peak_live = metrics.peak_live.max(self.live);
        metrics.total_bytes = self.total_bytes;
        metrics.peak_total_bytes = metrics.peak_total_bytes.max(self.total_bytes);
        metrics.issued = metrics.issued.saturating_add(1);
        self.metrics.set(metrics);
        Ok(BufferLease {
            engine: self.engine,
            provider_generation: self.provider_generation,
            consumer,
            handle: Handle { index, generation },
            artifact_id,
            len,
            deadline_millis,
        })
    }

    pub fn open_read(
        &self,
        lease: BufferLease,
        consumer: Handle,
        now_millis: u64,
    ) -> Result<(ArtifactId, usize), BufferLeaseError> {
        let record = self.validate(lease, consumer, now_millis)?;
        Ok((record.artifact_id, record.bytes.len()))
    }

    pub fn map_span(
        &self,
        lease: BufferLease,
        consumer: Handle,
        now_millis: u64,
        offset: usize,
        len: usize,
    ) -> Result<BufferSpan<'_>, BufferLeaseError> {
        if len > self.config.max_chunk_bytes {
            return Err(BufferLeaseError::ChunkCapacity);
        }
        let record = self.validate(lease, consumer, now_millis)?;
        let end = offset
            .checked_add(len)
            .ok_or(BufferLeaseError::OutOfBounds)?;
        Ok(BufferSpan(
            record
                .bytes
                .get(offset..end)
                .ok_or(BufferLeaseError::OutOfBounds)?,
        ))
    }

    pub fn read_chunk(
        &self,
        lease: BufferLease,
        consumer: Handle,
        now_millis: u64,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, BufferLeaseError> {
        Ok(self
            .map_span(lease, consumer, now_millis, offset, len)?
            .bytes()
            .to_vec())
    }

    pub fn verify_digest(
        &self,
        lease: BufferLease,
        consumer: Handle,
        now_millis: u64,
        expected: ArtifactId,
    ) -> Result<(), BufferLeaseError> {
        (self.validate(lease, consumer, now_millis)?.artifact_id == expected)
            .then_some(())
            .ok_or(BufferLeaseError::DigestMismatch)
    }

    pub fn release(
        &mut self,
        lease: BufferLease,
        consumer: Handle,
    ) -> Result<(), BufferLeaseError> {
        self.validate_identity(lease, consumer)?;
        self.remove(lease.handle.index)
    }

    pub fn cancel(&mut self, lease: BufferLease, consumer: Handle) -> Result<(), BufferLeaseError> {
        self.release(lease, consumer)
    }

    pub fn release_consumer(&mut self, consumer: Handle) -> Result<usize, BufferLeaseError> {
        self.ensure_open()?;
        if !consumer.is_valid() {
            return Err(BufferLeaseError::InvalidOwner);
        }
        let indices = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, item)| item.as_ref().is_some_and(|item| item.consumer == consumer))
            .map(|(index, _)| index as u32)
            .collect::<Vec<_>>();
        for index in &indices {
            self.remove(*index)?;
        }
        let mut metrics = self.metrics.get();
        metrics.expired = metrics.expired.saturating_add(indices.len() as u64);
        self.metrics.set(metrics);
        Ok(indices.len())
    }

    pub fn expire(&mut self, now_millis: u64) -> Result<usize, BufferLeaseError> {
        self.ensure_open()?;
        let indices = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.as_ref()
                    .is_some_and(|item| item.deadline_millis <= now_millis)
            })
            .map(|(index, _)| index as u32)
            .collect::<Vec<_>>();
        if indices
            .iter()
            .any(|index| self.generations[*index as usize] == u32::MAX)
        {
            return Err(BufferLeaseError::GenerationExhausted);
        }
        for index in &indices {
            self.remove(*index)?;
        }
        Ok(indices.len())
    }

    pub fn restart_provider(&mut self) -> Result<usize, BufferLeaseError> {
        self.ensure_open()?;
        self.preflight_restart_provider()?;
        let next_provider = self.provider_generation.generation + 1;
        let released = self.live;
        self.provider_generation.generation = next_provider;
        for index in 0..self.slots.len() {
            if self.slots[index].take().is_some() {
                self.generations[index] += 1;
                self.free.push(index as u32);
            }
        }
        self.live = 0;
        self.total_bytes = 0;
        let mut metrics = self.metrics.get();
        metrics.live = 0;
        metrics.total_bytes = 0;
        metrics.released = metrics.released.saturating_add(released as u64);
        metrics.provider_restarts = metrics.provider_restarts.saturating_add(1);
        self.metrics.set(metrics);
        Ok(released)
    }

    pub fn preflight_restart_provider(&self) -> Result<(), BufferLeaseError> {
        self.ensure_open()?;
        self.provider_generation
            .generation
            .checked_add(1)
            .ok_or(BufferLeaseError::GenerationExhausted)?;
        if self
            .slots
            .iter()
            .enumerate()
            .any(|(index, item)| item.is_some() && self.generations[index] == u32::MAX)
        {
            return Err(BufferLeaseError::GenerationExhausted);
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<usize, BufferLeaseError> {
        if self.closed {
            return Ok(0);
        }
        let released = self.live;
        self.closed = true;
        self.slots.clear();
        self.generations.clear();
        self.free.clear();
        self.live = 0;
        self.total_bytes = 0;
        let mut metrics = self.metrics.get();
        metrics.live = 0;
        metrics.total_bytes = 0;
        metrics.released = metrics.released.saturating_add(released as u64);
        self.metrics.set(metrics);
        Ok(released)
    }

    pub const fn live(&self) -> usize {
        self.live
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn metrics(&self) -> BufferLeaseMetrics {
        self.metrics.get()
    }

    fn validate(
        &self,
        lease: BufferLease,
        consumer: Handle,
        now_millis: u64,
    ) -> Result<&Record, BufferLeaseError> {
        let record = self.validate_identity(lease, consumer)?;
        if record.deadline_millis <= now_millis {
            return Err(BufferLeaseError::Expired);
        }
        Ok(record)
    }

    fn validate_identity(
        &self,
        lease: BufferLease,
        consumer: Handle,
    ) -> Result<&Record, BufferLeaseError> {
        self.ensure_open()?;
        if lease.engine != self.engine {
            self.record_stale_rejection();
            return Err(BufferLeaseError::WrongEngine);
        }
        if lease.provider_generation != self.provider_generation {
            self.record_stale_rejection();
            return Err(BufferLeaseError::StaleProvider);
        }
        if lease.consumer != consumer {
            self.record_stale_rejection();
            return Err(BufferLeaseError::WrongConsumer);
        }
        let Some(record) = self
            .slots
            .get(lease.handle.index as usize)
            .and_then(Option::as_ref)
        else {
            self.record_stale_rejection();
            return Err(BufferLeaseError::InvalidLease);
        };
        if record.generation != lease.handle.generation
            || record.consumer != consumer
            || record.artifact_id != lease.artifact_id
            || record.bytes.len() != lease.len
            || record.deadline_millis != lease.deadline_millis
        {
            self.record_stale_rejection();
            return Err(BufferLeaseError::InvalidLease);
        }
        Ok(record)
    }

    fn remove(&mut self, index: u32) -> Result<(), BufferLeaseError> {
        let next_generation = self.generations[index as usize]
            .checked_add(1)
            .ok_or(BufferLeaseError::GenerationExhausted)?;
        let record = self.slots[index as usize]
            .take()
            .ok_or(BufferLeaseError::InvalidLease)?;
        self.generations[index as usize] = next_generation;
        self.live -= 1;
        self.total_bytes -= record.bytes.len();
        self.free.push(index);
        let mut metrics = self.metrics.get();
        metrics.live = self.live;
        metrics.total_bytes = self.total_bytes;
        metrics.released = metrics.released.saturating_add(1);
        self.metrics.set(metrics);
        Ok(())
    }

    fn record_stale_rejection(&self) {
        let mut metrics = self.metrics.get();
        metrics.stale_rejections = metrics.stale_rejections.saturating_add(1);
        self.metrics.set(metrics);
    }

    fn ensure_open(&self) -> Result<(), BufferLeaseError> {
        if self.closed {
            Err(BufferLeaseError::Closed)
        } else {
            Ok(())
        }
    }
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

    fn registry() -> BufferLeaseRegistry {
        BufferLeaseRegistry::new(
            handle(1),
            handle(10),
            BufferLeaseConfig {
                max_leases: 2,
                max_total_bytes: 8,
                max_chunk_bytes: 3,
            },
        )
        .unwrap()
    }

    #[test]
    fn bounded_read_checks_owner_deadline_and_digest_before_exposure() {
        let mut registry = registry();
        let digest = ArtifactId([7; 16]);
        let lease = registry
            .issue(handle(20), digest, b"abcdef".to_vec(), 100)
            .unwrap();
        assert_eq!(registry.open_read(lease, handle(20), 1).unwrap().1, 6);
        assert_eq!(
            registry.read_chunk(lease, handle(20), 1, 2, 3).unwrap(),
            b"cde"
        );
        assert_eq!(
            registry.read_chunk(lease, handle(21), 1, 0, 1),
            Err(BufferLeaseError::WrongConsumer)
        );
        assert_eq!(
            registry.read_chunk(lease, handle(20), 1, 0, 4),
            Err(BufferLeaseError::ChunkCapacity)
        );
        assert_eq!(
            registry.verify_digest(lease, handle(20), 1, ArtifactId([8; 16])),
            Err(BufferLeaseError::DigestMismatch)
        );
        assert_eq!(
            registry.open_read(lease, handle(20), 100),
            Err(BufferLeaseError::Expired)
        );
    }

    #[test]
    fn capacity_release_and_provider_restart_preserve_generation_safety() {
        let mut registry = registry();
        let first = registry
            .issue(handle(20), ArtifactId([1; 16]), vec![1; 6], 100)
            .unwrap();
        assert_eq!(
            registry.issue(handle(21), ArtifactId([2; 16]), vec![2; 3], 100),
            Err(BufferLeaseError::Capacity)
        );
        assert_eq!(registry.total_bytes(), 6);
        registry.release(first, handle(20)).unwrap();
        let second = registry
            .issue(handle(21), ArtifactId([2; 16]), vec![2; 3], 100)
            .unwrap();
        assert_ne!(first.handle, second.handle);
        assert_eq!(registry.restart_provider().unwrap(), 1);
        assert_eq!(
            registry.open_read(second, handle(21), 1),
            Err(BufferLeaseError::StaleProvider)
        );
        assert_eq!(registry.live(), 0);
        assert_eq!(registry.total_bytes(), 0);
    }

    #[test]
    fn consumer_close_releases_only_its_leases() {
        let mut registry = registry();
        let first = registry
            .issue(handle(20), ArtifactId([1; 16]), vec![1], 100)
            .unwrap();
        let second = registry
            .issue(handle(21), ArtifactId([2; 16]), vec![2], 100)
            .unwrap();
        assert_eq!(registry.release_consumer(handle(20)).unwrap(), 1);
        assert_eq!(
            registry.open_read(first, handle(20), 1),
            Err(BufferLeaseError::InvalidLease)
        );
        assert_eq!(registry.open_read(second, handle(21), 1).unwrap().1, 1);
    }
}
