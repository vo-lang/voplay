use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::EngineId;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RenderAssetKey {
    pub kind: u32,
    pub asset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderAssetDispatch {
    pub key: RenderAssetKey,
    pub revision: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderAssetOutboxConfig {
    pub max_assets: usize,
    pub max_bytes: usize,
    pub max_inflight: usize,
}

impl Default for RenderAssetOutboxConfig {
    fn default() -> Self {
        Self {
            max_assets: 65_536,
            max_bytes: 1024 * 1024 * 1024,
            max_inflight: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderAssetOutboxError {
    InvalidConfig,
    InvalidAsset,
    StaleRevision,
    Capacity,
    ByteCapacity,
    InflightCapacity,
    UnknownAck,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderAssetOutboxOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub desired_assets: usize,
    pub pending_assets: usize,
    pub inflight_assets: usize,
    pub retained_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderAssetOutboxShutdownReport {
    pub released_assets: usize,
    pub released_pending: usize,
    pub released_inflight: usize,
    pub released_bytes: usize,
}

#[derive(Clone, Debug)]
struct DesiredAsset {
    revision: u64,
    payload: Vec<u8>,
    tombstone: bool,
    queued: bool,
}

pub struct RenderAssetOutbox {
    engine: EngineId,
    config: RenderAssetOutboxConfig,
    revisions: BTreeMap<RenderAssetKey, u64>,
    desired: BTreeMap<RenderAssetKey, DesiredAsset>,
    pending: VecDeque<RenderAssetKey>,
    inflight: BTreeSet<(RenderAssetKey, u64)>,
    bytes: usize,
    closed: bool,
}

impl RenderAssetOutbox {
    pub fn new(
        engine: EngineId,
        config: RenderAssetOutboxConfig,
    ) -> Result<Self, RenderAssetOutboxError> {
        if !engine.is_valid()
            || config.max_assets == 0
            || config.max_bytes == 0
            || config.max_inflight == 0
        {
            return Err(RenderAssetOutboxError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            revisions: BTreeMap::new(),
            desired: BTreeMap::new(),
            pending: VecDeque::new(),
            inflight: BTreeSet::new(),
            bytes: 0,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn stage(
        &mut self,
        key: RenderAssetKey,
        revision: u64,
        payload: Vec<u8>,
        tombstone: bool,
    ) -> Result<(), RenderAssetOutboxError> {
        self.ensure_open()?;
        if key.kind == 0
            || key.asset == 0
            || revision == 0
            || payload.is_empty()
            || self
                .revisions
                .get(&key)
                .is_some_and(|current| *current >= revision)
        {
            return Err(if self.revisions.contains_key(&key) {
                RenderAssetOutboxError::StaleRevision
            } else {
                RenderAssetOutboxError::InvalidAsset
            });
        }
        if !self.desired.contains_key(&key) && self.desired.len() == self.config.max_assets {
            return Err(RenderAssetOutboxError::Capacity);
        }
        let previous_bytes = self
            .desired
            .get(&key)
            .map_or(0, |asset| asset.payload.len());
        let next_bytes = self
            .bytes
            .checked_sub(previous_bytes)
            .and_then(|bytes| bytes.checked_add(payload.len()))
            .filter(|bytes| *bytes <= self.config.max_bytes)
            .ok_or(RenderAssetOutboxError::ByteCapacity)?;
        let was_queued = self.desired.get(&key).is_some_and(|asset| asset.queued);
        self.desired.insert(
            key,
            DesiredAsset {
                revision,
                payload,
                tombstone,
                queued: true,
            },
        );
        if !was_queued {
            self.pending.push_back(key);
        }
        self.revisions.insert(key, revision);
        self.bytes = next_bytes;
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<RenderAssetDispatch>, RenderAssetOutboxError> {
        self.ensure_open()?;
        if self.inflight.len() == self.config.max_inflight {
            return Err(RenderAssetOutboxError::InflightCapacity);
        }
        while let Some(key) = self.pending.pop_front() {
            let Some(asset) = self.desired.get_mut(&key) else {
                continue;
            };
            if !asset.queued {
                continue;
            }
            asset.queued = false;
            self.inflight.insert((key, asset.revision));
            return Ok(Some(RenderAssetDispatch {
                key,
                revision: asset.revision,
                payload: asset.payload.clone(),
            }));
        }
        Ok(None)
    }

    pub fn next_payload_len(&self) -> Option<usize> {
        self.pending.iter().find_map(|key| {
            self.desired
                .get(key)
                .filter(|asset| asset.queued)
                .map(|asset| asset.payload.len())
        })
    }

    pub fn has_dispatch_capacity(&self) -> bool {
        self.inflight.len() < self.config.max_inflight
    }

    pub fn acknowledge(
        &mut self,
        key: RenderAssetKey,
        revision: u64,
    ) -> Result<(), RenderAssetOutboxError> {
        self.ensure_open()?;
        if !self.inflight.remove(&(key, revision)) {
            return Err(RenderAssetOutboxError::UnknownAck);
        }
        let remove = self
            .desired
            .get(&key)
            .is_some_and(|asset| asset.revision == revision && asset.tombstone);
        if remove {
            if let Some(asset) = self.desired.remove(&key) {
                self.bytes -= asset.payload.len();
            }
        }
        Ok(())
    }

    pub fn begin_renderer_recovery(&mut self) -> Result<(), RenderAssetOutboxError> {
        self.ensure_open()?;
        self.inflight.clear();
        self.pending.clear();
        for (key, asset) in &mut self.desired {
            asset.queued = true;
            self.pending.push_back(*key);
        }
        Ok(())
    }

    pub const fn retained_bytes(&self) -> usize {
        self.bytes
    }

    pub fn owner_snapshot(&self) -> RenderAssetOutboxOwnerSnapshot {
        RenderAssetOutboxOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            desired_assets: self.desired.len(),
            pending_assets: self.pending.len(),
            inflight_assets: self.inflight.len(),
            retained_bytes: self.bytes,
        }
    }

    pub fn desired_count(&self) -> usize {
        self.desired.len()
    }

    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }

    pub fn is_synchronized(&self) -> bool {
        self.next_payload_len().is_none() && self.inflight.is_empty()
    }

    pub fn shutdown(&mut self) -> RenderAssetOutboxShutdownReport {
        if self.closed {
            return RenderAssetOutboxShutdownReport {
                released_assets: 0,
                released_pending: 0,
                released_inflight: 0,
                released_bytes: 0,
            };
        }
        let report = RenderAssetOutboxShutdownReport {
            released_assets: self.desired.len(),
            released_pending: self.pending.len(),
            released_inflight: self.inflight.len(),
            released_bytes: self.bytes,
        };
        self.revisions.clear();
        self.desired.clear();
        self.pending.clear();
        self.inflight.clear();
        self.bytes = 0;
        self.closed = true;
        report
    }

    fn ensure_open(&self) -> Result<(), RenderAssetOutboxError> {
        if self.closed {
            Err(RenderAssetOutboxError::Closed)
        } else {
            Ok(())
        }
    }
}
