use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::AssetRef,
    audio::PersistentSourceRecoveryPolicy,
    audio_mixer::{
        AudioBusDescriptor, AudioControlState, AudioDuckingRule, AudioListener, AudioSpatialSource,
        PersistentAudioSourceDescriptor,
    },
    control::{
        control_kind_tag, write_stable_binding, ControlDependencyRef, ControlDomain, ControlError,
        ControlKind, ControlSnapshotState, ControlStateSnapshot, ControlTxnBuilder,
        ControlTxnIdentity, DescriptorDependency, StableControlRef, STABLE_BINDING_BYTES,
    },
};

const MAGIC: [u8; 4] = *b"VAC1";
const BUS_TAG: u8 = 1;
const SOURCE_TAG: u8 = 2;
const UNITY_Q15: u16 = 32_767;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioControlDecodeConfig {
    pub max_entries: usize,
    pub max_descriptor_bytes: usize,
    pub max_buses: usize,
    pub max_sources: usize,
    pub max_ducking_rules: usize,
}

impl Default for AudioControlDecodeConfig {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_descriptor_bytes: 16 * 1024 * 1024,
            max_buses: 64,
            max_sources: 1024,
            max_ducking_rules: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioBusControlDescriptor {
    pub gain_q15: u16,
    pub mute: bool,
    pub solo: bool,
    pub listener: Option<AudioListener>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistentAudioControlDescriptor {
    pub asset: AssetRef,
    pub gain_q15: u16,
    pub spatial: Option<AudioSpatialSource>,
    pub looped: bool,
    pub recovery: PersistentSourceRecoveryPolicy,
    pub transport_anchor_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvisionalAudioBusRef {
    builder: ControlTxnIdentity,
    token: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvisionalPersistentAudioSourceRef {
    _token: u32,
}

impl ProvisionalAudioBusRef {
    pub const fn promotion_token(self) -> u32 {
        self.token
    }
}

impl ProvisionalPersistentAudioSourceRef {
    pub const fn promotion_token(self) -> u32 {
        self._token
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioBusBinding {
    Provisional(ProvisionalAudioBusRef),
    Stable(StableControlRef),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDuckingTargetBinding {
    pub target: AudioBusBinding,
    pub gain_q15: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioControlError {
    InvalidConfig,
    WrongEngine,
    WrongDomain,
    WrongBuilder,
    InvalidBus,
    InvalidSource,
    InvalidGain,
    InvalidListener,
    InvalidSpatial,
    DuckingCapacity,
    DuplicateDuckingTarget,
    Malformed,
    UnsupportedVersion,
    InvalidControlRevision,
    EntryCapacity,
    DescriptorCapacity,
    DuplicateEntry,
    MissingRootBus,
    MultipleRootBuses,
    MissingListener,
    MultipleListeners,
    UnknownBus,
    BusCycle,
    Control(ControlError),
}

impl From<ControlError> for AudioControlError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

pub struct AudioControlBuilder {
    engine: EngineId,
    identity: ControlTxnIdentity,
    issued_buses: BTreeSet<u32>,
    inner: ControlTxnBuilder,
}

impl AudioControlBuilder {
    pub fn new(engine: EngineId, inner: ControlTxnBuilder) -> Result<Self, AudioControlError> {
        if !engine.is_valid() {
            return Err(AudioControlError::WrongEngine);
        }
        let identity = inner.identity();
        Ok(Self {
            engine,
            identity,
            issued_buses: BTreeSet::new(),
            inner,
        })
    }

    pub fn create_bus(
        &mut self,
        parent: Option<AudioBusBinding>,
        descriptor: AudioBusControlDescriptor,
        ducking: &[AudioDuckingTargetBinding],
    ) -> Result<ProvisionalAudioBusRef, AudioControlError> {
        let encoded = encode_bus(
            self.engine,
            self.identity,
            &self.issued_buses,
            parent,
            descriptor,
            ducking,
        )?;
        let token = self.inner.create_with_dependencies(
            ControlKind::AudioBus,
            encoded.bytes,
            encoded.dependencies,
        )?;
        self.issued_buses.insert(token);
        Ok(ProvisionalAudioBusRef {
            builder: self.identity,
            token,
        })
    }

    pub fn create_persistent_source(
        &mut self,
        bus: AudioBusBinding,
        descriptor: PersistentAudioControlDescriptor,
    ) -> Result<ProvisionalPersistentAudioSourceRef, AudioControlError> {
        let encoded = encode_source(
            self.engine,
            self.identity,
            &self.issued_buses,
            bus,
            descriptor,
        )?;
        Ok(ProvisionalPersistentAudioSourceRef {
            _token: self.inner.create_with_dependencies(
                ControlKind::PersistentAudioSource,
                encoded.bytes,
                encoded.dependencies,
            )?,
        })
    }

    pub fn finish(self) -> ControlTxnBuilder {
        self.inner
    }
}

struct EncodedDescriptor {
    bytes: Vec<u8>,
    dependencies: Vec<DescriptorDependency>,
}

fn encode_bus(
    engine: EngineId,
    builder: ControlTxnIdentity,
    issued_buses: &BTreeSet<u32>,
    parent: Option<AudioBusBinding>,
    descriptor: AudioBusControlDescriptor,
    ducking: &[AudioDuckingTargetBinding],
) -> Result<EncodedDescriptor, AudioControlError> {
    validate_gain(descriptor.gain_q15)?;
    if let Some(listener) = descriptor.listener {
        validate_listener(listener)?;
    }
    if ducking.len() > u16::MAX as usize {
        return Err(AudioControlError::DuckingCapacity);
    }
    let mut bytes = descriptor_prefix(engine, BUS_TAG);
    let mut dependencies = Vec::new();
    match parent {
        Some(binding) => {
            bytes.push(1);
            encode_bus_binding(
                &mut bytes,
                &mut dependencies,
                engine,
                builder,
                issued_buses,
                binding,
            )?;
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(&descriptor.gain_q15.to_le_bytes());
    bytes.push(u8::from(descriptor.mute) | (u8::from(descriptor.solo) << 1));
    if let Some(listener) = descriptor.listener {
        bytes.push(1);
        encode_listener(&mut bytes, listener);
    } else {
        bytes.push(0);
    }
    bytes.extend_from_slice(&(ducking.len() as u16).to_le_bytes());
    let mut targets = BTreeSet::new();
    for rule in ducking {
        validate_gain(rule.gain_q15)?;
        let key = binding_identity(rule.target);
        if !targets.insert(key) {
            return Err(AudioControlError::DuplicateDuckingTarget);
        }
        encode_bus_binding(
            &mut bytes,
            &mut dependencies,
            engine,
            builder,
            issued_buses,
            rule.target,
        )?;
        bytes.extend_from_slice(&rule.gain_q15.to_le_bytes());
    }
    Ok(EncodedDescriptor {
        bytes,
        dependencies,
    })
}

fn encode_source(
    engine: EngineId,
    builder: ControlTxnIdentity,
    issued_buses: &BTreeSet<u32>,
    bus: AudioBusBinding,
    descriptor: PersistentAudioControlDescriptor,
) -> Result<EncodedDescriptor, AudioControlError> {
    if descriptor.asset.engine != engine || !descriptor.asset.handle.is_valid() {
        return Err(AudioControlError::InvalidSource);
    }
    validate_gain(descriptor.gain_q15)?;
    if let Some(spatial) = descriptor.spatial {
        validate_spatial(spatial)?;
    }
    let mut bytes = descriptor_prefix(engine, SOURCE_TAG);
    let mut dependencies = Vec::new();
    encode_bus_binding(
        &mut bytes,
        &mut dependencies,
        engine,
        builder,
        issued_buses,
        bus,
    )?;
    bytes.extend_from_slice(&descriptor.asset.handle.index.to_le_bytes());
    bytes.extend_from_slice(&descriptor.asset.handle.generation.to_le_bytes());
    bytes.extend_from_slice(&descriptor.gain_q15.to_le_bytes());
    let mut flags = u8::from(descriptor.looped);
    if descriptor.spatial.is_some() {
        flags |= 2;
    }
    bytes.push(flags);
    bytes.push(recovery_tag(descriptor.recovery));
    bytes.extend_from_slice(&descriptor.transport_anchor_millis.to_le_bytes());
    if let Some(spatial) = descriptor.spatial {
        encode_spatial(&mut bytes, spatial);
    }
    Ok(EncodedDescriptor {
        bytes,
        dependencies,
    })
}

fn descriptor_prefix(engine: EngineId, tag: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&MAGIC);
    bytes.push(tag);
    bytes.extend_from_slice(&engine.index.to_le_bytes());
    bytes.extend_from_slice(&engine.generation.to_le_bytes());
    bytes
}

fn encode_bus_binding(
    bytes: &mut Vec<u8>,
    dependencies: &mut Vec<DescriptorDependency>,
    engine: EngineId,
    builder: ControlTxnIdentity,
    issued_buses: &BTreeSet<u32>,
    binding: AudioBusBinding,
) -> Result<(), AudioControlError> {
    let offset = bytes.len();
    match binding {
        AudioBusBinding::Provisional(reference) => {
            if reference.builder != builder || !issued_buses.contains(&reference.token) {
                return Err(AudioControlError::WrongBuilder);
            }
            bytes.push(1);
            bytes.push(control_kind_tag(ControlKind::AudioBus));
            bytes.extend_from_slice(&reference.token.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            dependencies.push(DescriptorDependency {
                offset,
                reference: ControlDependencyRef::Provisional {
                    token: reference.token,
                    kind: ControlKind::AudioBus,
                },
            });
        }
        AudioBusBinding::Stable(stable) => {
            if stable.engine != engine {
                return Err(AudioControlError::WrongEngine);
            }
            if stable.kind != ControlKind::AudioBus || !stable.handle.is_valid() {
                return Err(AudioControlError::InvalidBus);
            }
            bytes.resize(offset + STABLE_BINDING_BYTES, 0);
            write_stable_binding(bytes, offset, stable)?;
            dependencies.push(DescriptorDependency {
                offset,
                reference: ControlDependencyRef::Stable(stable),
            });
        }
    }
    Ok(())
}

fn binding_identity(binding: AudioBusBinding) -> (u8, ControlTxnIdentity, u32, u32) {
    match binding {
        AudioBusBinding::Provisional(reference) => (1, reference.builder, reference.token, 0),
        AudioBusBinding::Stable(stable) => (
            2,
            ControlTxnIdentity {
                engine: stable.engine,
                lease: stable.handle,
                domain: ControlDomain::Audio,
                transaction_id: 0,
            },
            stable.handle.index,
            stable.handle.generation,
        ),
    }
}

pub fn decode_audio_control_snapshot(
    snapshot: &ControlStateSnapshot,
    config: AudioControlDecodeConfig,
) -> Result<AudioControlState, AudioControlError> {
    validate_decode_config(config)?;
    if !snapshot.engine.is_valid() {
        return Err(AudioControlError::WrongEngine);
    }
    if snapshot.domain != ControlDomain::Audio {
        return Err(AudioControlError::WrongDomain);
    }
    if snapshot.revision == 0 {
        return Err(AudioControlError::InvalidControlRevision);
    }
    if snapshot.entries.len() > config.max_entries {
        return Err(AudioControlError::EntryCapacity);
    }
    let mut descriptor_bytes = 0_usize;
    let mut seen = BTreeSet::new();
    let mut seen_handles = BTreeSet::new();
    let mut buses = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut ducking = Vec::new();
    let mut listener = None;
    for entry in &snapshot.entries {
        if entry.stable.engine != snapshot.engine || !entry.stable.handle.is_valid() {
            return Err(AudioControlError::WrongEngine);
        }
        if entry.stable.kind.domain() != ControlDomain::Audio {
            return Err(AudioControlError::WrongDomain);
        }
        if !seen.insert(entry.stable) || !seen_handles.insert(entry.stable.handle) {
            return Err(AudioControlError::DuplicateEntry);
        }
        descriptor_bytes = descriptor_bytes
            .checked_add(entry.descriptor.len())
            .filter(|bytes| *bytes <= config.max_descriptor_bytes)
            .ok_or(AudioControlError::DescriptorCapacity)?;
        if matches!(entry.state, ControlSnapshotState::Tombstone { .. }) {
            continue;
        }
        match entry.stable.kind {
            ControlKind::AudioBus => {
                if buses.len() == config.max_buses {
                    return Err(AudioControlError::EntryCapacity);
                }
                let decoded = decode_bus(snapshot.engine, entry.stable, &entry.descriptor, config)?;
                if let Some(decoded_listener) = decoded.listener {
                    if listener.replace((entry.stable, decoded_listener)).is_some() {
                        return Err(AudioControlError::MultipleListeners);
                    }
                }
                ducking.extend(decoded.ducking);
                buses.insert(entry.stable, decoded.descriptor);
            }
            ControlKind::PersistentAudioSource => {
                if sources.len() == config.max_sources {
                    return Err(AudioControlError::EntryCapacity);
                }
                let decoded = decode_source(snapshot.engine, entry.stable, &entry.descriptor)?;
                sources.insert(entry.stable, decoded);
            }
            _ => return Err(AudioControlError::WrongDomain),
        }
    }
    if ducking.len() > config.max_ducking_rules {
        return Err(AudioControlError::DuckingCapacity);
    }
    let roots = buses.values().filter(|bus| bus.parent.is_none()).count();
    if roots == 0 {
        return Err(AudioControlError::MissingRootBus);
    }
    if roots != 1 {
        return Err(AudioControlError::MultipleRootBuses);
    }
    let (listener_bus, listener) = listener.ok_or(AudioControlError::MissingListener)?;
    if buses
        .get(&listener_bus)
        .is_none_or(|descriptor| descriptor.parent.is_some())
    {
        return Err(AudioControlError::InvalidListener);
    }
    for bus in buses.values() {
        if bus
            .parent
            .is_some_and(|parent| !buses.contains_key(&parent))
        {
            return Err(AudioControlError::UnknownBus);
        }
        validate_bus_acyclic(bus.bus, &buses)?;
    }
    for source in sources.values() {
        if !buses.contains_key(&source.bus) {
            return Err(AudioControlError::UnknownBus);
        }
    }
    for rule in &ducking {
        if !buses.contains_key(&rule.trigger) || !buses.contains_key(&rule.target) {
            return Err(AudioControlError::UnknownBus);
        }
    }
    Ok(AudioControlState {
        engine: snapshot.engine,
        revision: snapshot.revision,
        buses: buses.into_values().collect(),
        sources: sources.into_values().collect(),
        ducking,
        listener,
    })
}

#[derive(Clone, Debug)]
struct DecodedBus {
    descriptor: AudioBusDescriptor,
    listener: Option<AudioListener>,
    ducking: Vec<AudioDuckingRule>,
}

fn decode_bus(
    engine: EngineId,
    stable: StableControlRef,
    bytes: &[u8],
    config: AudioControlDecodeConfig,
) -> Result<DecodedBus, AudioControlError> {
    let mut reader = Reader::new(bytes);
    reader.prefix(engine, BUS_TAG)?;
    let parent = match reader.u8()? {
        0 => None,
        1 => Some(reader.stable(engine, ControlKind::AudioBus)?),
        _ => return Err(AudioControlError::Malformed),
    };
    let gain_q15 = reader.u16()?;
    validate_gain(gain_q15)?;
    let flags = reader.u8()?;
    if flags & !3 != 0 {
        return Err(AudioControlError::Malformed);
    }
    let listener = match reader.u8()? {
        0 => None,
        1 => {
            let listener = reader.listener()?;
            validate_listener(listener)?;
            Some(listener)
        }
        _ => return Err(AudioControlError::Malformed),
    };
    let count = reader.u16()? as usize;
    if count > config.max_ducking_rules {
        return Err(AudioControlError::DuckingCapacity);
    }
    let mut ducking = Vec::with_capacity(count);
    let mut targets = BTreeSet::new();
    for _ in 0..count {
        let target = reader.stable(engine, ControlKind::AudioBus)?;
        let gain_q15 = reader.u16()?;
        validate_gain(gain_q15)?;
        if !targets.insert(target) {
            return Err(AudioControlError::DuplicateDuckingTarget);
        }
        ducking.push(AudioDuckingRule {
            trigger: stable,
            target,
            gain_q15,
        });
    }
    reader.finish()?;
    Ok(DecodedBus {
        descriptor: AudioBusDescriptor {
            bus: stable,
            parent,
            gain_q15,
            mute: flags & 1 != 0,
            solo: flags & 2 != 0,
        },
        listener,
        ducking,
    })
}

fn decode_source(
    engine: EngineId,
    stable: StableControlRef,
    bytes: &[u8],
) -> Result<PersistentAudioSourceDescriptor, AudioControlError> {
    let mut reader = Reader::new(bytes);
    reader.prefix(engine, SOURCE_TAG)?;
    let bus = reader.stable(engine, ControlKind::AudioBus)?;
    let asset = AssetRef {
        engine,
        handle: reader.handle()?,
    };
    let gain_q15 = reader.u16()?;
    validate_gain(gain_q15)?;
    let flags = reader.u8()?;
    if flags & !3 != 0 {
        return Err(AudioControlError::Malformed);
    }
    let recovery = match reader.u8()? {
        1 => PersistentSourceRecoveryPolicy::ResumeTimeline,
        2 => PersistentSourceRecoveryPolicy::Restart,
        3 => PersistentSourceRecoveryPolicy::StopOnRecovery,
        _ => return Err(AudioControlError::Malformed),
    };
    let transport_anchor_millis = reader.u64()?;
    let spatial = if flags & 2 != 0 {
        let spatial = reader.spatial()?;
        validate_spatial(spatial)?;
        Some(spatial)
    } else {
        None
    };
    reader.finish()?;
    Ok(PersistentAudioSourceDescriptor {
        source: stable,
        bus,
        asset,
        gain_q15,
        spatial,
        looped: flags & 1 != 0,
        recovery,
        transport_anchor_millis,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn prefix(&mut self, engine: EngineId, tag: u8) -> Result<(), AudioControlError> {
        if self.take(4)? != MAGIC || self.u8()? != tag {
            return Err(AudioControlError::UnsupportedVersion);
        }
        if self.u32()? != engine.index || self.u32()? != engine.generation {
            return Err(AudioControlError::WrongEngine);
        }
        Ok(())
    }

    fn stable(
        &mut self,
        engine: EngineId,
        kind: ControlKind,
    ) -> Result<StableControlRef, AudioControlError> {
        if self.u8()? != 2 || self.u8()? != control_kind_tag(kind) {
            return Err(AudioControlError::Malformed);
        }
        Ok(StableControlRef {
            engine,
            kind,
            handle: self.handle()?,
        })
    }

    fn handle(&mut self) -> Result<Handle, AudioControlError> {
        let handle = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !handle.is_valid() {
            return Err(AudioControlError::Malformed);
        }
        Ok(handle)
    }

    fn listener(&mut self) -> Result<AudioListener, AudioControlError> {
        Ok(AudioListener {
            position_mm: [self.i32()?, self.i32()?, self.i32()?],
            right_q15: [self.i16()?, self.i16()?, self.i16()?],
        })
    }

    fn spatial(&mut self) -> Result<AudioSpatialSource, AudioControlError> {
        Ok(AudioSpatialSource {
            position_mm: [self.i32()?, self.i32()?, self.i32()?],
            min_distance_mm: self.u32()?,
            max_distance_mm: self.u32()?,
        })
    }

    fn u8(&mut self) -> Result<u8, AudioControlError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, AudioControlError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn i16(&mut self) -> Result<i16, AudioControlError> {
        let bytes = self.take(2)?;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, AudioControlError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> Result<i32, AudioControlError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, AudioControlError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], AudioControlError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(AudioControlError::Malformed)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(AudioControlError::Malformed)?;
        self.offset = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<(), AudioControlError> {
        if self.offset != self.bytes.len() {
            return Err(AudioControlError::Malformed);
        }
        Ok(())
    }
}

fn encode_listener(bytes: &mut Vec<u8>, listener: AudioListener) {
    for component in listener.position_mm {
        bytes.extend_from_slice(&component.to_le_bytes());
    }
    for component in listener.right_q15 {
        bytes.extend_from_slice(&component.to_le_bytes());
    }
}

fn encode_spatial(bytes: &mut Vec<u8>, spatial: AudioSpatialSource) {
    for component in spatial.position_mm {
        bytes.extend_from_slice(&component.to_le_bytes());
    }
    bytes.extend_from_slice(&spatial.min_distance_mm.to_le_bytes());
    bytes.extend_from_slice(&spatial.max_distance_mm.to_le_bytes());
}

const fn recovery_tag(policy: PersistentSourceRecoveryPolicy) -> u8 {
    match policy {
        PersistentSourceRecoveryPolicy::ResumeTimeline => 1,
        PersistentSourceRecoveryPolicy::Restart => 2,
        PersistentSourceRecoveryPolicy::StopOnRecovery => 3,
    }
}

fn validate_decode_config(config: AudioControlDecodeConfig) -> Result<(), AudioControlError> {
    if config.max_entries == 0
        || config.max_descriptor_bytes == 0
        || config.max_buses == 0
        || config.max_sources == 0
        || config.max_ducking_rules == 0
    {
        return Err(AudioControlError::InvalidConfig);
    }
    Ok(())
}

fn validate_gain(gain: u16) -> Result<(), AudioControlError> {
    if gain > UNITY_Q15 {
        return Err(AudioControlError::InvalidGain);
    }
    Ok(())
}

fn validate_listener(listener: AudioListener) -> Result<(), AudioControlError> {
    if listener.right_q15 == [0; 3]
        || listener
            .right_q15
            .iter()
            .any(|component| *component == i16::MIN)
    {
        return Err(AudioControlError::InvalidListener);
    }
    Ok(())
}

fn validate_spatial(spatial: AudioSpatialSource) -> Result<(), AudioControlError> {
    if spatial.max_distance_mm == 0 || spatial.min_distance_mm > spatial.max_distance_mm {
        return Err(AudioControlError::InvalidSpatial);
    }
    Ok(())
}

fn validate_bus_acyclic(
    start: StableControlRef,
    buses: &BTreeMap<StableControlRef, AudioBusDescriptor>,
) -> Result<(), AudioControlError> {
    let mut visited = BTreeSet::new();
    let mut cursor = Some(start);
    while let Some(bus) = cursor {
        if !visited.insert(bus) {
            return Err(AudioControlError::BusCycle);
        }
        cursor = buses.get(&bus).and_then(|descriptor| descriptor.parent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        audio_mixer::{AudioMixerConfig, AudioMixerEndpoint},
        audio_streaming::{AudioStreamingConfig, AudioStreamingEndpoint},
        control::{ControlDomain, EngineControlConfig, EngineControlStore},
    };

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn store(engine: EngineId) -> EngineControlStore {
        EngineControlStore::new(
            engine,
            EngineControlConfig {
                max_leases: 8,
                max_refs: 16,
                max_ops_per_transaction: 8,
                max_descriptor_bytes: 4096,
                max_snapshot_bytes: 4096,
            },
        )
        .expect("store")
    }

    fn bus_descriptor(listener: bool) -> AudioBusControlDescriptor {
        AudioBusControlDescriptor {
            gain_q15: UNITY_Q15,
            mute: false,
            solo: false,
            listener: listener.then(AudioListener::default),
        }
    }

    fn source_descriptor(engine: EngineId) -> PersistentAudioControlDescriptor {
        PersistentAudioControlDescriptor {
            asset: AssetRef {
                engine,
                handle: handle(70),
            },
            gain_q15: UNITY_Q15,
            spatial: Some(AudioSpatialSource {
                position_mm: [100, 0, 0],
                min_distance_mm: 10,
                max_distance_mm: 1000,
            }),
            looped: true,
            recovery: PersistentSourceRecoveryPolicy::ResumeTimeline,
            transport_anchor_millis: 42,
        }
    }

    #[test]
    fn provisional_graph_commits_decodes_and_applies_to_mixer_end_to_end() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let lease = store.issue_lease(handle(8), logic, 3).expect("lease");
        let mut builder = AudioControlBuilder::new(engine, lease.begin(ControlDomain::Audio, 1, 0))
            .expect("builder");
        let root = builder
            .create_bus(None, bus_descriptor(true), &[])
            .expect("root");
        let music = builder
            .create_bus(
                Some(AudioBusBinding::Provisional(root)),
                bus_descriptor(false),
                &[AudioDuckingTargetBinding {
                    target: AudioBusBinding::Provisional(root),
                    gain_q15: (UNITY_Q15 + 1) / 2,
                }],
            )
            .expect("music");
        builder
            .create_persistent_source(
                AudioBusBinding::Provisional(music),
                source_descriptor(engine),
            )
            .expect("source");
        let committed = store
            .commit(builder.finish())
            .expect("commit")
            .publish_at_safe_point(logic)
            .expect("publish");
        assert_eq!(committed.promotions.len(), 3);
        let snapshot = store.snapshot(ControlDomain::Audio).expect("snapshot");
        let decoded = decode_audio_control_snapshot(&snapshot, AudioControlDecodeConfig::default())
            .expect("decode");
        assert_eq!(decoded.revision, 1);
        assert_eq!(decoded.buses.len(), 2);
        assert_eq!(decoded.sources.len(), 1);
        assert_eq!(decoded.ducking.len(), 1);
        assert_eq!(decoded.sources[0].transport_anchor_millis, 42);
        assert_eq!(decoded.sources[0].asset.handle, handle(70));
        let source_control = decoded.sources[0];

        let mut streaming = AudioStreamingEndpoint::new(
            engine,
            handle(50),
            handle(60),
            AudioStreamingConfig::default(),
        )
        .expect("streaming");
        let stream = streaming
            .create_stream_from_control(&source_control, 48_000, 2, 4096)
            .expect("stream from control");
        assert_eq!(
            streaming
                .request_decode(stream)
                .expect("decode work")
                .start_frame,
            2016
        );

        let mut mixer = AudioMixerEndpoint::new(engine, handle(50), AudioMixerConfig::default())
            .expect("mixer");
        mixer.apply_control_state(decoded).expect("apply");
        mixer
            .start_persistent(committed.promotions[2].1)
            .expect("realize source");
    }

    #[test]
    fn builder_local_refs_and_forged_stable_dependencies_fail_atomically() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let first_lease = store.issue_lease(handle(7), logic, 2).expect("first");
        let second_lease = store.issue_lease(handle(8), logic, 2).expect("second");
        let mut first =
            AudioControlBuilder::new(engine, first_lease.begin(ControlDomain::Audio, 1, 0))
                .expect("first builder");
        let foreign = first
            .create_bus(None, bus_descriptor(true), &[])
            .expect("foreign");
        let mut second =
            AudioControlBuilder::new(engine, second_lease.begin(ControlDomain::Audio, 1, 0))
                .expect("second builder");
        assert_eq!(
            second.create_persistent_source(
                AudioBusBinding::Provisional(foreign),
                source_descriptor(engine),
            ),
            Err(AudioControlError::WrongBuilder)
        );
        let forged = StableControlRef {
            engine,
            kind: ControlKind::AudioBus,
            handle: handle(12),
        };
        second
            .create_persistent_source(AudioBusBinding::Stable(forged), source_descriptor(engine))
            .expect("encoded forged dependency");
        assert!(matches!(
            store.commit(second.finish()),
            Err(ControlError::InvalidStableRef)
        ));
        assert_eq!(
            store
                .snapshot(ControlDomain::Audio)
                .expect("snapshot")
                .revision,
            0
        );
    }

    #[test]
    fn malformed_owner_and_quotas_fail_before_typed_state_creation() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let lease = store.issue_lease(handle(8), logic, 1).expect("lease");
        let mut builder = AudioControlBuilder::new(engine, lease.begin(ControlDomain::Audio, 1, 0))
            .expect("builder");
        builder
            .create_bus(None, bus_descriptor(true), &[])
            .expect("root");
        store.commit(builder.finish()).expect("commit");
        let snapshot = store.snapshot(ControlDomain::Audio).expect("snapshot");

        let mut truncated = snapshot.clone();
        truncated.entries[0].descriptor.pop();
        assert_eq!(
            decode_audio_control_snapshot(&truncated, AudioControlDecodeConfig::default()),
            Err(AudioControlError::Malformed)
        );
        let mut wrong_owner = snapshot.clone();
        wrong_owner.entries[0].stable.engine = handle(2);
        assert_eq!(
            decode_audio_control_snapshot(&wrong_owner, AudioControlDecodeConfig::default()),
            Err(AudioControlError::WrongEngine)
        );
        assert_eq!(
            decode_audio_control_snapshot(
                &snapshot,
                AudioControlDecodeConfig {
                    max_descriptor_bytes: 1,
                    ..AudioControlDecodeConfig::default()
                },
            ),
            Err(AudioControlError::DescriptorCapacity)
        );
    }
}
