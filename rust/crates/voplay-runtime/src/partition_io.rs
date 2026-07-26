use std::{
    fs::File,
    io::{Read, Take},
    path::PathBuf,
};

use voplay_protocol::{EngineId, Handle};

use crate::world_partition::PartitionId;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PartitionIoSource {
    File { path: PathBuf },
    Http { url: String },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PartitionIoRequestId {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionIoRequest {
    pub partition: PartitionId,
    pub source: PartitionIoSource,
    pub priority: i32,
    pub deadline_millis: u64,
    pub max_bytes: usize,
    pub expected_digest: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionIoWork {
    pub request: PartitionIoRequestId,
    pub provider_generation: Handle,
    pub actor: Handle,
    pub partition: PartitionId,
    pub source: PartitionIoSource,
    pub deadline_millis: u64,
    pub max_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionIoWorkerError {
    UnsupportedSource,
    OpenFailed,
    ReadFailed,
    TooLarge,
}

pub trait PartitionIoWorker {
    fn execute(&mut self, work: &PartitionIoWork) -> Result<Vec<u8>, PartitionIoWorkerError>;
}

#[derive(Default)]
pub struct NativeFilePartitionIoWorker;

impl PartitionIoWorker for NativeFilePartitionIoWorker {
    fn execute(&mut self, work: &PartitionIoWork) -> Result<Vec<u8>, PartitionIoWorkerError> {
        let PartitionIoSource::File { path } = &work.source else {
            return Err(PartitionIoWorkerError::UnsupportedSource);
        };
        let file = File::open(path).map_err(|_| PartitionIoWorkerError::OpenFailed)?;
        let limit = u64::try_from(work.max_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or(PartitionIoWorkerError::TooLarge)?;
        let mut reader: Take<File> = file.take(limit);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| PartitionIoWorkerError::ReadFailed)?;
        if bytes.len() > work.max_bytes {
            return Err(PartitionIoWorkerError::TooLarge);
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionIoOutcome {
    Ready,
    Cancelled,
    TimedOut,
    Failed(PartitionIoWorkerError),
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionIoResult {
    pub request: PartitionIoRequestId,
    pub partition: PartitionId,
    pub outcome: PartitionIoOutcome,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionIoConfig {
    pub max_requests: usize,
    pub max_reserved_bytes: usize,
    pub max_source_bytes: usize,
}

impl Default for PartitionIoConfig {
    fn default() -> Self {
        Self {
            max_requests: 4096,
            max_reserved_bytes: 256 * 1024 * 1024,
            max_source_bytes: 16 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionIoState {
    Running,
    Closing,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartitionIoError {
    InvalidConfig,
    InvalidIdentity,
    RequestCapacity,
    ByteCapacity,
    SourceCapacity,
    InvalidRequest,
    StaleProvider,
    WrongActor,
    InvalidState,
    WorkNotDispatched,
    DuplicateCompletion,
    LengthMismatch,
    DigestMismatch,
    DrainPending,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionIoOwnerSnapshot {
    pub state: PartitionIoState,
    pub provider_generation: Handle,
    pub live_requests: usize,
    pub pending_requests: usize,
    pub dispatched_requests: usize,
    pub terminal_requests: usize,
    pub reserved_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionIoShutdownReport {
    pub released_requests: usize,
    pub released_pending: usize,
    pub released_dispatched: usize,
    pub released_terminal: usize,
    pub released_reserved_bytes: usize,
    pub invalidated_provider_generation: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionIoRequestStatus {
    Pending,
    Dispatched,
    Terminal(PartitionIoOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestState {
    Pending,
    Dispatched,
    Terminal,
}

#[derive(Clone, Debug)]
struct RequestRecord {
    request: PartitionIoRequest,
    state: RequestState,
    terminal_request: Option<PartitionIoOutcome>,
    outcome: Option<PartitionIoOutcome>,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct RequestSlot {
    generation: u32,
    record: Option<RequestRecord>,
}

pub struct PartitionIoScheduler {
    engine: EngineId,
    actor: Handle,
    provider_generation: Handle,
    config: PartitionIoConfig,
    state: PartitionIoState,
    slots: Vec<RequestSlot>,
    free: Vec<u32>,
    reserved_bytes: usize,
}

impl PartitionIoScheduler {
    pub fn new(
        engine: EngineId,
        actor: Handle,
        provider_generation: Handle,
        config: PartitionIoConfig,
    ) -> Result<Self, PartitionIoError> {
        if !engine.is_valid()
            || !actor.is_valid()
            || !provider_generation.is_valid()
            || config.max_requests == 0
            || config.max_requests > u32::MAX as usize
            || config.max_reserved_bytes == 0
            || config.max_source_bytes == 0
        {
            return Err(PartitionIoError::InvalidConfig);
        }
        Ok(Self {
            engine,
            actor,
            provider_generation,
            config,
            state: PartitionIoState::Running,
            slots: Vec::new(),
            free: Vec::new(),
            reserved_bytes: 0,
        })
    }

    pub const fn state(&self) -> PartitionIoState {
        self.state
    }

    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    pub fn owner_snapshot(&self) -> PartitionIoOwnerSnapshot {
        let mut pending_requests = 0usize;
        let mut dispatched_requests = 0usize;
        let mut terminal_requests = 0usize;
        for record in self.slots.iter().filter_map(|slot| slot.record.as_ref()) {
            match record.state {
                RequestState::Pending => pending_requests += 1,
                RequestState::Dispatched => dispatched_requests += 1,
                RequestState::Terminal => terminal_requests += 1,
            }
        }
        PartitionIoOwnerSnapshot {
            state: self.state,
            provider_generation: self.provider_generation,
            live_requests: pending_requests + dispatched_requests + terminal_requests,
            pending_requests,
            dispatched_requests,
            terminal_requests,
            reserved_bytes: self.reserved_bytes,
        }
    }

    pub fn request_status(
        &self,
        id: PartitionIoRequestId,
    ) -> Result<PartitionIoRequestStatus, PartitionIoError> {
        let record = self.record(id)?;
        Ok(match record.state {
            RequestState::Pending => PartitionIoRequestStatus::Pending,
            RequestState::Dispatched => PartitionIoRequestStatus::Dispatched,
            RequestState::Terminal => PartitionIoRequestStatus::Terminal(
                record.outcome.ok_or(PartitionIoError::InvalidState)?,
            ),
        })
    }

    pub fn submit(
        &mut self,
        request: PartitionIoRequest,
    ) -> Result<PartitionIoRequestId, PartitionIoError> {
        if self.state != PartitionIoState::Running {
            return Err(PartitionIoError::InvalidState);
        }
        validate_request(&request, &self.config)?;
        let live = self
            .slots
            .iter()
            .filter(|slot| slot.record.is_some())
            .count();
        if live >= self.config.max_requests {
            return Err(PartitionIoError::RequestCapacity);
        }
        let reserved_bytes = self
            .reserved_bytes
            .checked_add(request.max_bytes)
            .filter(|bytes| *bytes <= self.config.max_reserved_bytes)
            .ok_or(PartitionIoError::ByteCapacity)?;
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            let index =
                u32::try_from(self.slots.len()).map_err(|_| PartitionIoError::RequestCapacity)?;
            self.slots.push(RequestSlot {
                generation: 1,
                record: None,
            });
            index
        };
        let slot = &mut self.slots[index as usize];
        let id = PartitionIoRequestId {
            engine: self.engine,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.record = Some(RequestRecord {
            request,
            state: RequestState::Pending,
            terminal_request: None,
            outcome: None,
            bytes: Vec::new(),
        });
        self.reserved_bytes = reserved_bytes;
        Ok(id)
    }

    pub fn poll_work(&mut self, now_millis: u64) -> Option<PartitionIoWork> {
        let mut candidate: Option<(usize, i32)> = None;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(record) = slot.record.as_mut() else {
                continue;
            };
            if record.state == RequestState::Pending && record.request.deadline_millis <= now_millis
            {
                record.state = RequestState::Terminal;
                record.outcome = Some(PartitionIoOutcome::TimedOut);
                continue;
            }
            if record.state == RequestState::Pending
                && candidate.is_none_or(|(_, priority)| record.request.priority > priority)
            {
                candidate = Some((index, record.request.priority));
            }
        }
        let (index, _) = candidate?;
        let slot = &mut self.slots[index];
        let record = slot.record.as_mut()?;
        record.state = RequestState::Dispatched;
        Some(PartitionIoWork {
            request: PartitionIoRequestId {
                engine: self.engine,
                handle: Handle {
                    index: index as u32,
                    generation: slot.generation,
                },
            },
            provider_generation: self.provider_generation,
            actor: self.actor,
            partition: record.request.partition,
            source: record.request.source.clone(),
            deadline_millis: record.request.deadline_millis,
            max_bytes: record.request.max_bytes,
        })
    }

    pub fn complete(
        &mut self,
        work: PartitionIoWork,
        completion: Result<Vec<u8>, PartitionIoWorkerError>,
        now_millis: u64,
    ) -> Result<PartitionIoOutcome, PartitionIoError> {
        self.validate_work(&work)?;
        let record = self.record_mut(work.request)?;
        if record.state != RequestState::Dispatched {
            return Err(if record.state == RequestState::Terminal {
                PartitionIoError::DuplicateCompletion
            } else {
                PartitionIoError::WorkNotDispatched
            });
        }
        let (outcome, bytes) = if let Some(outcome) = record.terminal_request {
            (outcome, Vec::new())
        } else if record.request.deadline_millis <= now_millis {
            (PartitionIoOutcome::TimedOut, Vec::new())
        } else {
            match completion {
                Err(error) => (PartitionIoOutcome::Failed(error), Vec::new()),
                Ok(bytes) => {
                    if bytes.len() > record.request.max_bytes {
                        return Err(PartitionIoError::LengthMismatch);
                    }
                    if record
                        .request
                        .expected_digest
                        .is_some_and(|expected| stable_digest(&bytes) != expected)
                    {
                        return Err(PartitionIoError::DigestMismatch);
                    }
                    (PartitionIoOutcome::Ready, bytes)
                }
            }
        };
        record.state = RequestState::Terminal;
        record.outcome = Some(outcome);
        record.bytes = bytes;
        Ok(outcome)
    }

    pub fn cancel(
        &mut self,
        id: PartitionIoRequestId,
    ) -> Result<Option<PartitionIoOutcome>, PartitionIoError> {
        let record = self.record_mut(id)?;
        match record.state {
            RequestState::Pending => {
                record.state = RequestState::Terminal;
                record.outcome = Some(PartitionIoOutcome::Cancelled);
                Ok(Some(PartitionIoOutcome::Cancelled))
            }
            RequestState::Dispatched => {
                if record.terminal_request.is_none() {
                    record.terminal_request = Some(PartitionIoOutcome::Cancelled);
                }
                Ok(None)
            }
            RequestState::Terminal => Ok(record.outcome),
        }
    }

    pub fn expire(&mut self, now_millis: u64) -> Vec<PartitionIoRequestId> {
        let mut expired = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(record) = slot.record.as_mut() else {
                continue;
            };
            if record.request.deadline_millis > now_millis {
                continue;
            }
            let id = PartitionIoRequestId {
                engine: self.engine,
                handle: Handle {
                    index: index as u32,
                    generation: slot.generation,
                },
            };
            match record.state {
                RequestState::Pending => {
                    record.state = RequestState::Terminal;
                    record.outcome = Some(PartitionIoOutcome::TimedOut);
                    expired.push(id);
                }
                RequestState::Dispatched => {
                    if record.terminal_request.is_none() {
                        record.terminal_request = Some(PartitionIoOutcome::TimedOut);
                        expired.push(id);
                    }
                }
                RequestState::Terminal => {}
            }
        }
        expired
    }

    pub fn take_result(
        &mut self,
        id: PartitionIoRequestId,
    ) -> Result<PartitionIoResult, PartitionIoError> {
        let index = self.request_index(id)?;
        let record = self.slots[index]
            .record
            .as_ref()
            .ok_or(PartitionIoError::InvalidRequest)?;
        if record.state != RequestState::Terminal {
            return Err(PartitionIoError::WorkNotDispatched);
        }
        let next_generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(PartitionIoError::GenerationExhausted)?;
        let record = self.slots[index].record.take().unwrap();
        self.reserved_bytes -= record.request.max_bytes;
        self.slots[index].generation = next_generation;
        self.free.push(index as u32);
        Ok(PartitionIoResult {
            request: id,
            partition: record.request.partition,
            outcome: record.outcome.unwrap(),
            bytes: record.bytes,
        })
    }

    pub fn restart_provider(&mut self) -> Result<(), PartitionIoError> {
        if self.state != PartitionIoState::Running {
            return Err(PartitionIoError::InvalidState);
        }
        self.provider_generation.generation = self
            .provider_generation
            .generation
            .checked_add(1)
            .ok_or(PartitionIoError::GenerationExhausted)?;
        for slot in &mut self.slots {
            if let Some(record) = &mut slot.record {
                if record.state == RequestState::Dispatched {
                    if let Some(outcome) = record.terminal_request {
                        record.state = RequestState::Terminal;
                        record.outcome = Some(outcome);
                    } else {
                        record.state = RequestState::Pending;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn begin_close(&mut self) -> Result<(), PartitionIoError> {
        if self.state != PartitionIoState::Running {
            return Err(PartitionIoError::InvalidState);
        }
        self.state = PartitionIoState::Closing;
        for slot in &mut self.slots {
            if let Some(record) = &mut slot.record {
                match record.state {
                    RequestState::Pending => {
                        record.state = RequestState::Terminal;
                        record.outcome = Some(PartitionIoOutcome::Closed);
                    }
                    RequestState::Dispatched => {
                        record.terminal_request = Some(PartitionIoOutcome::Closed)
                    }
                    RequestState::Terminal => {}
                }
            }
        }
        Ok(())
    }

    pub fn finish_close(&mut self) -> Result<(), PartitionIoError> {
        if self.state != PartitionIoState::Closing {
            return Err(PartitionIoError::InvalidState);
        }
        if self.slots.iter().any(|slot| {
            slot.record
                .as_ref()
                .is_some_and(|record| record.state == RequestState::Dispatched)
        }) {
            return Err(PartitionIoError::DrainPending);
        }
        self.state = PartitionIoState::Closed;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<PartitionIoShutdownReport, PartitionIoError> {
        if self.state == PartitionIoState::Closed && self.reserved_bytes == 0 {
            return Ok(PartitionIoShutdownReport {
                released_requests: 0,
                released_pending: 0,
                released_dispatched: 0,
                released_terminal: 0,
                released_reserved_bytes: 0,
                invalidated_provider_generation: self.provider_generation,
            });
        }
        let before = self.owner_snapshot();
        let invalidated_provider_generation = Handle {
            index: self.provider_generation.index,
            generation: self
                .provider_generation
                .generation
                .checked_add(1)
                .ok_or(PartitionIoError::GenerationExhausted)?,
        };
        for slot in &self.slots {
            slot.generation
                .checked_add(1)
                .ok_or(PartitionIoError::GenerationExhausted)?;
        }
        self.provider_generation = invalidated_provider_generation;
        self.state = PartitionIoState::Closed;
        self.slots.clear();
        self.free.clear();
        self.reserved_bytes = 0;
        Ok(PartitionIoShutdownReport {
            released_requests: before.live_requests,
            released_pending: before.pending_requests,
            released_dispatched: before.dispatched_requests,
            released_terminal: before.terminal_requests,
            released_reserved_bytes: before.reserved_bytes,
            invalidated_provider_generation,
        })
    }

    pub fn drain_terminal_results(&mut self) -> Result<Vec<PartitionIoResult>, PartitionIoError> {
        if self.slots.iter().any(|slot| {
            slot.record
                .as_ref()
                .is_some_and(|record| record.state != RequestState::Terminal)
        }) {
            return Err(PartitionIoError::DrainPending);
        }
        for slot in self.slots.iter().filter(|slot| slot.record.is_some()) {
            slot.generation
                .checked_add(1)
                .ok_or(PartitionIoError::GenerationExhausted)?;
        }
        let ids = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.record.as_ref().map(|_| PartitionIoRequestId {
                    engine: self.engine,
                    handle: Handle {
                        index: index as u32,
                        generation: slot.generation,
                    },
                })
            })
            .collect::<Vec<_>>();
        ids.into_iter().map(|id| self.take_result(id)).collect()
    }

    fn validate_work(&self, work: &PartitionIoWork) -> Result<(), PartitionIoError> {
        if work.request.engine != self.engine {
            return Err(PartitionIoError::InvalidIdentity);
        }
        if work.actor != self.actor {
            return Err(PartitionIoError::WrongActor);
        }
        if work.provider_generation != self.provider_generation {
            return Err(PartitionIoError::StaleProvider);
        }
        let record = self.record(work.request)?;
        if work.partition != record.request.partition
            || work.source != record.request.source
            || work.deadline_millis != record.request.deadline_millis
            || work.max_bytes != record.request.max_bytes
        {
            return Err(PartitionIoError::InvalidIdentity);
        }
        Ok(())
    }

    fn request_index(&self, id: PartitionIoRequestId) -> Result<usize, PartitionIoError> {
        if id.engine != self.engine {
            return Err(PartitionIoError::InvalidIdentity);
        }
        let index = id.handle.index as usize;
        let slot = self
            .slots
            .get(index)
            .ok_or(PartitionIoError::InvalidRequest)?;
        if id.handle.generation == 0
            || id.handle.generation != slot.generation
            || slot.record.is_none()
        {
            return Err(PartitionIoError::InvalidRequest);
        }
        Ok(index)
    }

    fn record(&self, id: PartitionIoRequestId) -> Result<&RequestRecord, PartitionIoError> {
        let index = self.request_index(id)?;
        self.slots[index]
            .record
            .as_ref()
            .ok_or(PartitionIoError::InvalidRequest)
    }

    fn record_mut(
        &mut self,
        id: PartitionIoRequestId,
    ) -> Result<&mut RequestRecord, PartitionIoError> {
        let index = self.request_index(id)?;
        self.slots[index]
            .record
            .as_mut()
            .ok_or(PartitionIoError::InvalidRequest)
    }
}

fn validate_request(
    request: &PartitionIoRequest,
    config: &PartitionIoConfig,
) -> Result<(), PartitionIoError> {
    if !request.partition.is_valid() || request.max_bytes == 0 || request.deadline_millis == 0 {
        return Err(PartitionIoError::InvalidIdentity);
    }
    let source_bytes = match &request.source {
        PartitionIoSource::File { path } => path.as_os_str().as_encoded_bytes().len(),
        PartitionIoSource::Http { url } => {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(PartitionIoError::InvalidIdentity);
            }
            url.len()
        }
    };
    if source_bytes == 0 || source_bytes > config.max_source_bytes {
        return Err(PartitionIoError::SourceCapacity);
    }
    Ok(())
}

pub fn stable_digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn engine() -> EngineId {
        handle(1)
    }

    fn scheduler(config: PartitionIoConfig) -> PartitionIoScheduler {
        PartitionIoScheduler::new(engine(), handle(2), handle(3), config).unwrap()
    }

    fn request(source: PartitionIoSource, priority: i32, max_bytes: usize) -> PartitionIoRequest {
        PartitionIoRequest {
            partition: PartitionId(1),
            source,
            priority,
            deadline_millis: 100,
            max_bytes,
            expected_digest: None,
        }
    }

    #[test]
    fn native_file_worker_reads_real_bytes_and_scheduler_exposes_only_verified_result() {
        let bytes = b"world-partition-native-file".to_vec();
        let unique = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "voplay-partition-io-{}-{unique}.bin",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let mut request = request(PartitionIoSource::File { path: path.clone() }, 1, 1024);
        request.expected_digest = Some(stable_digest(&bytes));
        let mut scheduler = scheduler(PartitionIoConfig::default());
        let id = scheduler.submit(request).unwrap();
        let work = scheduler.poll_work(0).unwrap();
        let mut worker = NativeFilePartitionIoWorker;
        let completion = worker.execute(&work);
        assert_eq!(
            scheduler.complete(work, completion, 1).unwrap(),
            PartitionIoOutcome::Ready
        );
        let result = scheduler.take_result(id).unwrap();
        assert_eq!(result.bytes, bytes);
        assert_eq!(scheduler.reserved_bytes(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn provider_restart_rejects_late_work_and_digest_failure_exposes_no_bytes() {
        let bytes = b"verified".to_vec();
        let mut request = request(
            PartitionIoSource::Http {
                url: "https://example.invalid/chunk".to_owned(),
            },
            1,
            64,
        );
        request.expected_digest = Some(stable_digest(&bytes));
        let mut scheduler = scheduler(PartitionIoConfig::default());
        let id = scheduler.submit(request).unwrap();
        let stale = scheduler.poll_work(0).unwrap();
        scheduler.restart_provider().unwrap();
        assert_eq!(
            scheduler.complete(stale, Ok(bytes.clone()), 1),
            Err(PartitionIoError::StaleProvider)
        );
        let current = scheduler.poll_work(1).unwrap();
        assert_eq!(
            scheduler.complete(current.clone(), Ok(b"corrupt".to_vec()), 2),
            Err(PartitionIoError::DigestMismatch)
        );
        assert_eq!(
            scheduler.complete(current, Ok(bytes.clone()), 2).unwrap(),
            PartitionIoOutcome::Ready
        );
        assert_eq!(scheduler.take_result(id).unwrap().bytes, bytes);
    }

    #[test]
    fn priority_and_capacity_failures_are_stable_and_consume_no_handle() {
        let mut config = PartitionIoConfig::default();
        config.max_requests = 2;
        config.max_reserved_bytes = 8;
        let source = PartitionIoSource::Http {
            url: "https://example.invalid/chunk".to_owned(),
        };
        let mut scheduler = scheduler(config);
        let low = scheduler.submit(request(source.clone(), 1, 4)).unwrap();
        let high = scheduler.submit(request(source.clone(), 10, 4)).unwrap();
        assert_eq!(
            scheduler.submit(request(source.clone(), 20, 1)),
            Err(PartitionIoError::RequestCapacity)
        );
        assert_eq!(scheduler.poll_work(0).unwrap().request, high);
        scheduler.cancel(high).unwrap();
        let high_work = PartitionIoWork {
            request: high,
            provider_generation: handle(3),
            actor: handle(2),
            partition: PartitionId(1),
            source: source.clone(),
            deadline_millis: 100,
            max_bytes: 4,
        };
        scheduler.complete(high_work, Ok(Vec::new()), 1).unwrap();
        scheduler.take_result(high).unwrap();
        scheduler.cancel(low).unwrap();
        scheduler.take_result(low).unwrap();
        let reused = scheduler.submit(request(source, 5, 8)).unwrap();
        assert_eq!(reused.handle.index, low.handle.index);
        assert_ne!(reused.handle.generation, low.handle.generation);
    }

    #[test]
    fn close_cancels_pending_and_drains_dispatched_before_closed() {
        let source = PartitionIoSource::Http {
            url: "https://example.invalid/chunk".to_owned(),
        };
        let mut scheduler = scheduler(PartitionIoConfig::default());
        let dispatched = scheduler.submit(request(source.clone(), 10, 16)).unwrap();
        let pending = scheduler.submit(request(source, 1, 16)).unwrap();
        let work = scheduler.poll_work(0).unwrap();
        assert_eq!(work.request, dispatched);
        scheduler.begin_close().unwrap();
        assert_eq!(
            scheduler.finish_close(),
            Err(PartitionIoError::DrainPending)
        );
        assert_eq!(
            scheduler.complete(work, Ok(vec![1, 2, 3]), 1).unwrap(),
            PartitionIoOutcome::Closed
        );
        scheduler.finish_close().unwrap();
        assert_eq!(scheduler.state(), PartitionIoState::Closed);
        assert_eq!(
            scheduler.take_result(dispatched).unwrap().outcome,
            PartitionIoOutcome::Closed
        );
        assert_eq!(
            scheduler.take_result(pending).unwrap().outcome,
            PartitionIoOutcome::Closed
        );
        assert_eq!(scheduler.reserved_bytes(), 0);
    }

    #[test]
    fn native_worker_rejects_http_and_oversized_file_without_partial_bytes() {
        let mut worker = NativeFilePartitionIoWorker;
        let http = PartitionIoWork {
            request: PartitionIoRequestId {
                engine: engine(),
                handle: handle(1),
            },
            provider_generation: handle(3),
            actor: handle(2),
            partition: PartitionId(1),
            source: PartitionIoSource::Http {
                url: "https://example.invalid/chunk".to_owned(),
            },
            deadline_millis: 10,
            max_bytes: 1,
        };
        assert_eq!(
            worker.execute(&http),
            Err(PartitionIoWorkerError::UnsupportedSource)
        );

        let unique = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "voplay-partition-io-large-{}-{unique}.bin",
            std::process::id()
        ));
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let mut file_work = http;
        file_work.source = PartitionIoSource::File { path: path.clone() };
        assert_eq!(
            worker.execute(&file_work),
            Err(PartitionIoWorkerError::TooLarge)
        );
        std::fs::remove_file(path).unwrap();
    }
}
