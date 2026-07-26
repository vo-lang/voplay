use std::collections::BTreeMap;

use voplay_protocol::EngineId;

use crate::outbox::PresentationDomainId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationStateConfig {
    pub max_domains: usize,
    pub max_entries_per_domain: usize,
    pub max_value_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for PresentationStateConfig {
    fn default() -> Self {
        Self {
            max_domains: 64,
            max_entries_per_domain: 4096,
            max_value_bytes: 64 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresentationStateOp {
    Set { key: u64, value: Vec<u8> },
    Remove { key: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationStateError {
    Closed,
    InvalidConfig,
    InvalidDomain,
    InvalidFrame,
    InvalidKey,
    DomainCapacity,
    EntryCapacity,
    ValueCapacity,
    TotalCapacity,
    FrameSequence,
    RevisionExhausted,
    SnapshotIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationDomainSnapshot {
    pub domain: PresentationDomainId,
    pub revision: u64,
    pub last_frame_id: u64,
    pub values: BTreeMap<u64, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationStateSnapshot {
    pub engine: EngineId,
    pub domains: Vec<PresentationDomainSnapshot>,
}

#[derive(Clone, Debug)]
struct DomainState {
    revision: u64,
    last_frame_id: u64,
    values: BTreeMap<u64, Vec<u8>>,
    bytes: usize,
}

pub struct PresentationStateCommit {
    domain: PresentationDomainId,
    state: DomainState,
    total_bytes: usize,
}

pub struct PresentationState {
    engine: EngineId,
    config: PresentationStateConfig,
    domains: BTreeMap<PresentationDomainId, DomainState>,
    total_bytes: usize,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationStateOwnerSnapshot {
    pub closed: bool,
    pub domains: usize,
    pub entries: usize,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationStateShutdownReport {
    pub released_domains: usize,
    pub released_entries: usize,
    pub released_bytes: usize,
}

impl PresentationState {
    pub fn new(
        engine: EngineId,
        config: PresentationStateConfig,
    ) -> Result<Self, PresentationStateError> {
        if !engine.is_valid()
            || config.max_domains == 0
            || config.max_entries_per_domain == 0
            || config.max_value_bytes == 0
            || config.max_total_bytes == 0
        {
            return Err(PresentationStateError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            domains: BTreeMap::new(),
            total_bytes: 0,
            closed: false,
        })
    }

    pub fn value(&self, domain: PresentationDomainId, key: u64) -> Option<&[u8]> {
        if domain.engine != self.engine || key == 0 {
            return None;
        }
        self.domains
            .get(&domain)?
            .values
            .get(&key)
            .map(Vec::as_slice)
    }

    pub fn domain_revision(&self, domain: PresentationDomainId) -> Option<u64> {
        (domain.engine == self.engine)
            .then(|| self.domains.get(&domain).map(|state| state.revision))
            .flatten()
    }

    pub fn snapshot(&self) -> PresentationStateSnapshot {
        PresentationStateSnapshot {
            engine: self.engine,
            domains: self
                .domains
                .iter()
                .map(|(domain, state)| PresentationDomainSnapshot {
                    domain: *domain,
                    revision: state.revision,
                    last_frame_id: state.last_frame_id,
                    values: state.values.clone(),
                })
                .collect(),
        }
    }

    pub fn owner_snapshot(&self) -> PresentationStateOwnerSnapshot {
        PresentationStateOwnerSnapshot {
            closed: self.closed,
            domains: self.domains.len(),
            entries: self.domains.values().map(|state| state.values.len()).sum(),
            bytes: self.total_bytes,
        }
    }

    pub fn shutdown(&mut self) -> PresentationStateShutdownReport {
        if self.closed {
            return PresentationStateShutdownReport {
                released_domains: 0,
                released_entries: 0,
                released_bytes: 0,
            };
        }
        let report = PresentationStateShutdownReport {
            released_domains: self.domains.len(),
            released_entries: self.domains.values().map(|state| state.values.len()).sum(),
            released_bytes: self.total_bytes,
        };
        self.closed = true;
        self.domains.clear();
        self.total_bytes = 0;
        report
    }

    pub fn restore(
        &mut self,
        snapshot: PresentationStateSnapshot,
    ) -> Result<(), PresentationStateError> {
        self.ensure_open()?;
        if snapshot.engine != self.engine || snapshot.domains.len() > self.config.max_domains {
            return Err(PresentationStateError::SnapshotIdentity);
        }
        let mut domains = BTreeMap::new();
        let mut total_bytes = 0_usize;
        for domain in snapshot.domains {
            if domain.domain.engine != self.engine
                || !domain.domain.handle.is_valid()
                || domain.last_frame_id == 0
                || domain.values.len() > self.config.max_entries_per_domain
                || domains.contains_key(&domain.domain)
            {
                return Err(PresentationStateError::SnapshotIdentity);
            }
            let bytes = domain
                .values
                .iter()
                .try_fold(0_usize, |total, (key, value)| {
                    if *key == 0 || value.len() > self.config.max_value_bytes {
                        return None;
                    }
                    total.checked_add(value.len())
                })
                .ok_or(PresentationStateError::SnapshotIdentity)?;
            total_bytes = total_bytes
                .checked_add(bytes)
                .filter(|total| *total <= self.config.max_total_bytes)
                .ok_or(PresentationStateError::SnapshotIdentity)?;
            domains.insert(
                domain.domain,
                DomainState {
                    revision: domain.revision,
                    last_frame_id: domain.last_frame_id,
                    values: domain.values,
                    bytes,
                },
            );
        }
        self.domains = domains;
        self.total_bytes = total_bytes;
        Ok(())
    }

    pub fn prepare_frame(
        &self,
        domain: PresentationDomainId,
        frame_id: u64,
        ops: Vec<PresentationStateOp>,
    ) -> Result<PresentationStateCommit, PresentationStateError> {
        self.ensure_open()?;
        if domain.engine != self.engine || !domain.handle.is_valid() {
            return Err(PresentationStateError::InvalidDomain);
        }
        if frame_id == 0 {
            return Err(PresentationStateError::InvalidFrame);
        }
        let existing = self.domains.get(&domain);
        if existing.is_none() && self.domains.len() == self.config.max_domains {
            return Err(PresentationStateError::DomainCapacity);
        }
        if existing.is_some_and(|state| frame_id <= state.last_frame_id) {
            return Err(PresentationStateError::FrameSequence);
        }
        let mut staged = existing.cloned().unwrap_or(DomainState {
            revision: 0,
            last_frame_id: 0,
            values: BTreeMap::new(),
            bytes: 0,
        });
        let before = staged.values.clone();
        for op in ops {
            match op {
                PresentationStateOp::Set { key, value } => {
                    if key == 0 {
                        return Err(PresentationStateError::InvalidKey);
                    }
                    if value.len() > self.config.max_value_bytes {
                        return Err(PresentationStateError::ValueCapacity);
                    }
                    staged.values.insert(key, value);
                }
                PresentationStateOp::Remove { key } => {
                    if key == 0 {
                        return Err(PresentationStateError::InvalidKey);
                    }
                    staged.values.remove(&key);
                }
            }
        }
        if staged.values.len() > self.config.max_entries_per_domain {
            return Err(PresentationStateError::EntryCapacity);
        }
        staged.bytes = staged
            .values
            .values()
            .try_fold(0usize, |total, value| total.checked_add(value.len()))
            .ok_or(PresentationStateError::TotalCapacity)?;
        let total_bytes = self
            .total_bytes
            .checked_sub(existing.map_or(0, |state| state.bytes))
            .and_then(|total| total.checked_add(staged.bytes))
            .filter(|total| *total <= self.config.max_total_bytes)
            .ok_or(PresentationStateError::TotalCapacity)?;
        staged.last_frame_id = frame_id;
        if staged.values != before {
            staged.revision = staged
                .revision
                .checked_add(1)
                .ok_or(PresentationStateError::RevisionExhausted)?;
        }
        Ok(PresentationStateCommit {
            domain,
            state: staged,
            total_bytes,
        })
    }

    pub fn commit(
        &mut self,
        commit: PresentationStateCommit,
    ) -> Result<(), PresentationStateError> {
        self.ensure_open()?;
        self.total_bytes = commit.total_bytes;
        self.domains.insert(commit.domain, commit.state);
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), PresentationStateError> {
        if self.closed {
            Err(PresentationStateError::Closed)
        } else {
            Ok(())
        }
    }
}
