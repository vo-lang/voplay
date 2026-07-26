use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::asset::AssetRef;
use crate::audio::PersistentSourceRecoveryPolicy;
use crate::control::{ControlKind, StableControlRef};

const UNITY_Q15: u16 = 32_767;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioMixerConfig {
    pub max_buses: usize,
    pub max_sources: usize,
    pub max_ducking_rules: usize,
    pub max_voices: usize,
    pub max_callback_frames: usize,
}

impl Default for AudioMixerConfig {
    fn default() -> Self {
        Self {
            max_buses: 64,
            max_sources: 1024,
            max_ducking_rules: 128,
            max_voices: 1024,
            max_callback_frames: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioBusDescriptor {
    pub bus: StableControlRef,
    pub parent: Option<StableControlRef>,
    pub gain_q15: u16,
    pub mute: bool,
    pub solo: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDuckingRule {
    pub trigger: StableControlRef,
    pub target: StableControlRef,
    pub gain_q15: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioSpatialSource {
    pub position_mm: [i32; 3],
    pub min_distance_mm: u32,
    pub max_distance_mm: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioListener {
    pub position_mm: [i32; 3],
    pub right_q15: [i16; 3],
}

impl Default for AudioListener {
    fn default() -> Self {
        Self {
            position_mm: [0; 3],
            right_q15: [UNITY_Q15 as i16, 0, 0],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistentAudioSourceDescriptor {
    pub source: StableControlRef,
    pub bus: StableControlRef,
    pub asset: AssetRef,
    pub gain_q15: u16,
    pub spatial: Option<AudioSpatialSource>,
    pub looped: bool,
    pub recovery: PersistentSourceRecoveryPolicy,
    pub transport_anchor_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioControlState {
    pub engine: EngineId,
    pub revision: u64,
    pub buses: Vec<AudioBusDescriptor>,
    pub sources: Vec<PersistentAudioSourceDescriptor>,
    pub ducking: Vec<AudioDuckingRule>,
    pub listener: AudioListener,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AudioVoiceHandle {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioVoiceKind {
    OneShot { event_id: u64, asset: AssetRef },
    Persistent { source: StableControlRef },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioOneShotDescriptor {
    pub event_id: u64,
    pub asset: AssetRef,
    pub bus: StableControlRef,
    pub gain_q15: u16,
    pub spatial: Option<AudioSpatialSource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioMixInput<'a> {
    pub voice: AudioVoiceHandle,
    pub channels: u16,
    pub samples: &'a [i16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioMixReport {
    pub control_revision: u64,
    pub frames: usize,
    pub submitted_voices: usize,
    pub audible_voices: usize,
    pub clipped_samples: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioMixerOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub control_revision: u64,
    pub buses: usize,
    pub persistent_sources: usize,
    pub ducking_rules: usize,
    pub active_voices: usize,
    pub voice_slots: usize,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioMixerShutdownReport {
    pub endpoint_generation: Handle,
    pub released_buses: usize,
    pub released_sources: usize,
    pub released_ducking_rules: usize,
    pub released_voices: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioMixerError {
    InvalidConfig,
    WrongEngine,
    StaleEndpoint,
    InvalidControlRevision,
    BusCapacity,
    SourceCapacity,
    DuckingCapacity,
    DuplicateBus,
    DuplicateSource,
    InvalidBus,
    InvalidSource,
    UnknownBus,
    UnknownSource,
    BusCycle,
    InvalidGain,
    InvalidSpatial,
    InvalidListener,
    ControlInUse,
    VoiceCapacity,
    UnknownVoice,
    DuplicateVoiceInput,
    InputShape,
    CallbackShape,
    CallbackCapacity,
    SpatialRequiresMono,
    GenerationExhausted,
    EndpointGenerationExhausted,
    ArithmeticOverflow,
    Closed,
}

#[derive(Clone, Copy, Debug)]
struct VoiceSlot {
    kind: AudioVoiceKind,
    bus: StableControlRef,
    gain_q15: u16,
    spatial: Option<AudioSpatialSource>,
    transport_frames: u64,
}

pub struct AudioMixerEndpoint {
    engine: EngineId,
    endpoint_generation: Handle,
    config: AudioMixerConfig,
    control_revision: u64,
    buses: BTreeMap<StableControlRef, AudioBusDescriptor>,
    sources: BTreeMap<StableControlRef, PersistentAudioSourceDescriptor>,
    ducking: Vec<AudioDuckingRule>,
    listener: AudioListener,
    voices: Vec<Option<VoiceSlot>>,
    voice_generations: Vec<u32>,
    free_voices: BTreeSet<u32>,
    closed: bool,
}

impl AudioMixerEndpoint {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: AudioMixerConfig,
    ) -> Result<Self, AudioMixerError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_buses == 0
            || config.max_sources == 0
            || config.max_ducking_rules == 0
            || config.max_voices == 0
            || config.max_callback_frames == 0
        {
            return Err(AudioMixerError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            control_revision: 0,
            buses: BTreeMap::new(),
            sources: BTreeMap::new(),
            ducking: Vec::new(),
            listener: AudioListener::default(),
            voices: Vec::new(),
            voice_generations: Vec::new(),
            free_voices: BTreeSet::new(),
            closed: false,
        })
    }

    pub const fn endpoint_generation(&self) -> Handle {
        self.endpoint_generation
    }

    pub const fn control_revision(&self) -> u64 {
        self.control_revision
    }

    pub fn owner_snapshot(&self) -> AudioMixerOwnerSnapshot {
        AudioMixerOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            control_revision: self.control_revision,
            buses: self.buses.len(),
            persistent_sources: self.sources.len(),
            ducking_rules: self.ducking.len(),
            active_voices: self.active_voice_count(),
            voice_slots: self.voices.len(),
            closed: self.closed,
        }
    }

    pub fn bus_count(&self) -> usize {
        self.buses.len()
    }

    pub fn persistent_source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().flatten().count()
    }

    pub fn persistent_sources(
        &self,
    ) -> impl ExactSizeIterator<Item = &PersistentAudioSourceDescriptor> {
        self.sources.values()
    }

    pub fn persistent_source(
        &self,
        source: StableControlRef,
    ) -> Option<&PersistentAudioSourceDescriptor> {
        self.sources.get(&source)
    }

    pub fn apply_control_state(&mut self, state: AudioControlState) -> Result<(), AudioMixerError> {
        self.apply_control_state_inner(state, false)
    }

    #[cfg(feature = "audio")]
    pub(crate) fn replace_control_state_at_current_revision(
        &mut self,
        state: AudioControlState,
    ) -> Result<(), AudioMixerError> {
        self.apply_control_state_inner(state, true)
    }

    fn apply_control_state_inner(
        &mut self,
        state: AudioControlState,
        allow_same_revision: bool,
    ) -> Result<(), AudioMixerError> {
        self.ensure_open()?;
        if state.engine != self.engine {
            return Err(AudioMixerError::WrongEngine);
        }
        if state.revision == 0
            || state.revision < self.control_revision
            || state.revision == self.control_revision && !allow_same_revision
        {
            return Err(AudioMixerError::InvalidControlRevision);
        }
        if state.buses.is_empty() || state.buses.len() > self.config.max_buses {
            return Err(AudioMixerError::BusCapacity);
        }
        if state.sources.len() > self.config.max_sources {
            return Err(AudioMixerError::SourceCapacity);
        }
        if state.ducking.len() > self.config.max_ducking_rules {
            return Err(AudioMixerError::DuckingCapacity);
        }
        validate_listener(state.listener)?;

        let mut buses = BTreeMap::new();
        for bus in state.buses {
            validate_control_ref(self.engine, bus.bus, ControlKind::AudioBus)
                .map_err(|_| AudioMixerError::InvalidBus)?;
            validate_gain(bus.gain_q15)?;
            if let Some(parent) = bus.parent {
                validate_control_ref(self.engine, parent, ControlKind::AudioBus)
                    .map_err(|_| AudioMixerError::InvalidBus)?;
                if parent == bus.bus {
                    return Err(AudioMixerError::BusCycle);
                }
            }
            if buses.insert(bus.bus, bus).is_some() {
                return Err(AudioMixerError::DuplicateBus);
            }
        }
        for bus in buses.values() {
            if bus
                .parent
                .is_some_and(|parent| !buses.contains_key(&parent))
            {
                return Err(AudioMixerError::UnknownBus);
            }
            validate_bus_acyclic(bus.bus, &buses)?;
        }

        let mut sources = BTreeMap::new();
        for source in state.sources {
            validate_control_ref(
                self.engine,
                source.source,
                ControlKind::PersistentAudioSource,
            )
            .map_err(|_| AudioMixerError::InvalidSource)?;
            validate_control_ref(self.engine, source.bus, ControlKind::AudioBus)
                .map_err(|_| AudioMixerError::InvalidBus)?;
            if source.asset.engine != self.engine || !source.asset.handle.is_valid() {
                return Err(AudioMixerError::InvalidSource);
            }
            validate_gain(source.gain_q15)?;
            if !buses.contains_key(&source.bus) {
                return Err(AudioMixerError::UnknownBus);
            }
            if let Some(spatial) = source.spatial {
                validate_spatial(spatial)?;
            }
            if sources.insert(source.source, source).is_some() {
                return Err(AudioMixerError::DuplicateSource);
            }
        }

        for rule in &state.ducking {
            validate_control_ref(self.engine, rule.trigger, ControlKind::AudioBus)
                .map_err(|_| AudioMixerError::InvalidBus)?;
            validate_control_ref(self.engine, rule.target, ControlKind::AudioBus)
                .map_err(|_| AudioMixerError::InvalidBus)?;
            validate_gain(rule.gain_q15)?;
            if !buses.contains_key(&rule.trigger) || !buses.contains_key(&rule.target) {
                return Err(AudioMixerError::UnknownBus);
            }
        }
        let mut ducking_keys = BTreeSet::new();
        for rule in &state.ducking {
            if !ducking_keys.insert((rule.trigger, rule.target)) {
                return Err(AudioMixerError::DuckingCapacity);
            }
        }

        let mut staged_voice_updates = Vec::with_capacity(self.voices.iter().flatten().count());
        let mut removed_persistent = Vec::new();
        let mut active_persistent = BTreeSet::new();
        for (index, voice) in self.voices.iter().enumerate() {
            let Some(voice) = voice else {
                continue;
            };
            let updated = match voice.kind {
                AudioVoiceKind::OneShot { .. } => {
                    if !buses.contains_key(&voice.bus) {
                        return Err(AudioMixerError::ControlInUse);
                    }
                    *voice
                }
                AudioVoiceKind::Persistent { source } => {
                    if let Some(descriptor) = sources.get(&source) {
                        active_persistent.insert(source);
                        VoiceSlot {
                            bus: descriptor.bus,
                            gain_q15: descriptor.gain_q15,
                            spatial: descriptor.spatial,
                            ..*voice
                        }
                    } else {
                        self.voice_generations[index]
                            .checked_add(1)
                            .ok_or(AudioMixerError::GenerationExhausted)?;
                        removed_persistent.push(index);
                        continue;
                    }
                }
            };
            staged_voice_updates.push((index, updated));
        }
        let added_persistent = sources
            .keys()
            .filter(|source| !active_persistent.contains(source))
            .copied()
            .collect::<Vec<_>>();
        let active_after_removal = self
            .voices
            .iter()
            .flatten()
            .count()
            .saturating_sub(removed_persistent.len());
        if active_after_removal
            .checked_add(added_persistent.len())
            .is_none_or(|voices| voices > self.config.max_voices)
        {
            return Err(AudioMixerError::VoiceCapacity);
        }

        self.buses = buses;
        self.sources = sources;
        self.ducking = state.ducking;
        self.listener = state.listener;
        self.control_revision = state.revision;
        for (index, voice) in staged_voice_updates {
            self.voices[index] = Some(voice);
        }
        for index in removed_persistent {
            self.voice_generations[index] += 1;
            self.voices[index] = None;
            self.free_voices.insert(index as u32);
        }
        for source in added_persistent {
            let descriptor = self.sources[&source];
            self.allocate_voice(VoiceSlot {
                kind: AudioVoiceKind::Persistent { source },
                bus: descriptor.bus,
                gain_q15: descriptor.gain_q15,
                spatial: descriptor.spatial,
                transport_frames: 0,
            })?;
        }
        Ok(())
    }

    pub fn start_one_shot(
        &mut self,
        descriptor: AudioOneShotDescriptor,
    ) -> Result<AudioVoiceHandle, AudioMixerError> {
        self.ensure_open()?;
        if descriptor.event_id == 0 {
            return Err(AudioMixerError::InvalidSource);
        }
        if descriptor.asset.engine != self.engine || !descriptor.asset.handle.is_valid() {
            return Err(AudioMixerError::InvalidSource);
        }
        validate_control_ref(self.engine, descriptor.bus, ControlKind::AudioBus)
            .map_err(|_| AudioMixerError::InvalidBus)?;
        if !self.buses.contains_key(&descriptor.bus) {
            return Err(AudioMixerError::UnknownBus);
        }
        validate_gain(descriptor.gain_q15)?;
        if let Some(spatial) = descriptor.spatial {
            validate_spatial(spatial)?;
        }
        self.allocate_voice(VoiceSlot {
            kind: AudioVoiceKind::OneShot {
                event_id: descriptor.event_id,
                asset: descriptor.asset,
            },
            bus: descriptor.bus,
            gain_q15: descriptor.gain_q15,
            spatial: descriptor.spatial,
            transport_frames: 0,
        })
    }

    pub fn start_persistent(
        &mut self,
        source: StableControlRef,
    ) -> Result<AudioVoiceHandle, AudioMixerError> {
        self.ensure_open()?;
        validate_control_ref(self.engine, source, ControlKind::PersistentAudioSource)
            .map_err(|_| AudioMixerError::InvalidSource)?;
        if let Some(voice) = self.persistent_voice(source) {
            return Ok(voice);
        }
        let descriptor = self
            .sources
            .get(&source)
            .copied()
            .ok_or(AudioMixerError::UnknownSource)?;
        self.allocate_voice(VoiceSlot {
            kind: AudioVoiceKind::Persistent { source },
            bus: descriptor.bus,
            gain_q15: descriptor.gain_q15,
            spatial: descriptor.spatial,
            transport_frames: 0,
        })
    }

    pub fn persistent_voice(&self, source: StableControlRef) -> Option<AudioVoiceHandle> {
        self.voices.iter().enumerate().find_map(|(index, voice)| {
            voice
                .as_ref()
                .is_some_and(|voice| {
                    matches!(
                        voice.kind,
                        AudioVoiceKind::Persistent { source: active } if active == source
                    )
                })
                .then_some(AudioVoiceHandle {
                    engine: self.engine,
                    endpoint_generation: self.endpoint_generation,
                    handle: Handle {
                        index: index as u32,
                        generation: self.voice_generations[index],
                    },
                })
        })
    }

    pub fn stop_voice(&mut self, voice: AudioVoiceHandle) -> Result<(), AudioMixerError> {
        self.ensure_open()?;
        let index = self.validate_voice_index(voice)?;
        let generation = self.voice_generations[index]
            .checked_add(1)
            .ok_or(AudioMixerError::GenerationExhausted)?;
        self.voice_generations[index] = generation;
        self.voices[index] = None;
        self.free_voices.insert(index as u32);
        Ok(())
    }

    pub fn preflight_stop_voices(
        &self,
        voices: &[AudioVoiceHandle],
    ) -> Result<(), AudioMixerError> {
        self.ensure_open()?;
        let mut unique = BTreeSet::new();
        for voice in voices {
            let index = self.validate_voice_index(*voice)?;
            if !unique.insert(*voice) {
                return Err(AudioMixerError::DuplicateVoiceInput);
            }
            self.voice_generations[index]
                .checked_add(1)
                .ok_or(AudioMixerError::GenerationExhausted)?;
        }
        Ok(())
    }

    pub fn voice_transport_frames(&self, voice: AudioVoiceHandle) -> Result<u64, AudioMixerError> {
        let index = self.validate_voice_index(voice)?;
        Ok(self.voices[index]
            .as_ref()
            .ok_or(AudioMixerError::UnknownVoice)?
            .transport_frames)
    }

    pub fn mix_stereo_into(
        &mut self,
        inputs: &[AudioMixInput<'_>],
        output: &mut [i16],
    ) -> Result<AudioMixReport, AudioMixerError> {
        self.ensure_open()?;
        if output.len() % 2 != 0 {
            return Err(AudioMixerError::CallbackShape);
        }
        let frames = output.len() / 2;
        if frames > self.config.max_callback_frames || inputs.len() > self.config.max_voices {
            return Err(AudioMixerError::CallbackCapacity);
        }
        for (position, input) in inputs.iter().enumerate() {
            let index = self.validate_voice_index(input.voice)?;
            let voice = self.voices[index]
                .as_ref()
                .ok_or(AudioMixerError::UnknownVoice)?;
            if input.channels != 1 && input.channels != 2 {
                return Err(AudioMixerError::InputShape);
            }
            let expected = frames
                .checked_mul(input.channels as usize)
                .ok_or(AudioMixerError::ArithmeticOverflow)?;
            if input.samples.len() != expected {
                return Err(AudioMixerError::InputShape);
            }
            if voice.spatial.is_some() && input.channels != 1 {
                return Err(AudioMixerError::SpatialRequiresMono);
            }
            voice
                .transport_frames
                .checked_add(frames as u64)
                .ok_or(AudioMixerError::ArithmeticOverflow)?;
            if inputs[..position]
                .iter()
                .any(|previous| previous.voice == input.voice)
            {
                return Err(AudioMixerError::DuplicateVoiceInput);
            }
        }

        let solo_active = self.buses.values().any(|bus| bus.solo);
        let mut audible_voices = 0;
        for input in inputs {
            let voice = self.voice(input.voice);
            if self.voice_gain_q15(voice, inputs, solo_active) != 0 {
                audible_voices += 1;
            }
        }

        let mut clipped_samples = 0;
        for frame in 0..frames {
            let mut left = 0_i64;
            let mut right = 0_i64;
            for input in inputs {
                let voice = self.voice(input.voice);
                let bus_gain = self.voice_gain_q15(voice, inputs, solo_active);
                if bus_gain == 0 {
                    continue;
                }
                if let Some(spatial) = voice.spatial {
                    let (attenuation, left_pan, right_pan) = spatial_gains(self.listener, spatial);
                    let gain = multiply_q15(bus_gain, attenuation);
                    let sample = input.samples[frame] as i64;
                    left += apply_gain(sample, multiply_q15(gain, left_pan));
                    right += apply_gain(sample, multiply_q15(gain, right_pan));
                } else if input.channels == 1 {
                    let sample = apply_gain(input.samples[frame] as i64, bus_gain);
                    left += sample;
                    right += sample;
                } else {
                    left += apply_gain(input.samples[frame * 2] as i64, bus_gain);
                    right += apply_gain(input.samples[frame * 2 + 1] as i64, bus_gain);
                }
            }
            let (left_sample, left_clipped) = clamp_i16(left);
            let (right_sample, right_clipped) = clamp_i16(right);
            output[frame * 2] = left_sample;
            output[frame * 2 + 1] = right_sample;
            clipped_samples += usize::from(left_clipped) + usize::from(right_clipped);
        }
        for input in inputs {
            let index = input.voice.handle.index as usize;
            let voice = self.voices[index]
                .as_mut()
                .ok_or(AudioMixerError::UnknownVoice)?;
            voice.transport_frames += frames as u64;
        }
        Ok(AudioMixReport {
            control_revision: self.control_revision,
            frames,
            submitted_voices: inputs.len(),
            audible_voices,
            clipped_samples,
        })
    }

    pub fn restart(&mut self) -> Result<(), AudioMixerError> {
        self.ensure_open()?;
        let generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioMixerError::EndpointGenerationExhausted)?;
        self.endpoint_generation.generation = generation;
        self.voices.clear();
        self.voice_generations.clear();
        self.free_voices.clear();
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<AudioMixerShutdownReport, AudioMixerError> {
        if self.closed {
            return Ok(AudioMixerShutdownReport {
                endpoint_generation: self.endpoint_generation,
                released_buses: 0,
                released_sources: 0,
                released_ducking_rules: 0,
                released_voices: 0,
            });
        }
        self.preflight_shutdown()?;
        let generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioMixerError::EndpointGenerationExhausted)?;
        let report = AudioMixerShutdownReport {
            endpoint_generation: Handle {
                index: self.endpoint_generation.index,
                generation,
            },
            released_buses: self.buses.len(),
            released_sources: self.sources.len(),
            released_ducking_rules: self.ducking.len(),
            released_voices: self.active_voice_count(),
        };
        self.endpoint_generation.generation = generation;
        self.control_revision = 0;
        self.buses.clear();
        self.sources.clear();
        self.ducking.clear();
        self.listener = AudioListener::default();
        self.voices.clear();
        self.voice_generations.clear();
        self.free_voices.clear();
        self.closed = true;
        Ok(report)
    }

    pub fn preflight_shutdown(&self) -> Result<(), AudioMixerError> {
        if self.closed {
            return Ok(());
        }
        self.endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioMixerError::EndpointGenerationExhausted)?;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), AudioMixerError> {
        if self.closed {
            Err(AudioMixerError::Closed)
        } else {
            Ok(())
        }
    }

    fn allocate_voice(&mut self, slot: VoiceSlot) -> Result<AudioVoiceHandle, AudioMixerError> {
        let index = if let Some(index) = self.free_voices.first().copied() {
            index
        } else {
            if self.voices.len() == self.config.max_voices || self.voices.len() >= u32::MAX as usize
            {
                return Err(AudioMixerError::VoiceCapacity);
            }
            self.voices.len() as u32
        };
        let index_usize = index as usize;
        let generation = if index_usize == self.voices.len() {
            self.voices.push(None);
            self.voice_generations.push(1);
            1
        } else {
            self.voice_generations[index_usize]
        };
        self.free_voices.remove(&index);
        self.voices[index_usize] = Some(slot);
        Ok(AudioVoiceHandle {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle: Handle { index, generation },
        })
    }

    fn validate_voice_index(&self, voice: AudioVoiceHandle) -> Result<usize, AudioMixerError> {
        if voice.engine != self.engine {
            return Err(AudioMixerError::WrongEngine);
        }
        if voice.endpoint_generation != self.endpoint_generation {
            return Err(AudioMixerError::StaleEndpoint);
        }
        if !voice.handle.is_valid() {
            return Err(AudioMixerError::UnknownVoice);
        }
        let index = voice.handle.index as usize;
        if self.voice_generations.get(index).copied() != Some(voice.handle.generation)
            || self.voices.get(index).and_then(Option::as_ref).is_none()
        {
            return Err(AudioMixerError::UnknownVoice);
        }
        Ok(index)
    }

    fn voice(&self, handle: AudioVoiceHandle) -> &VoiceSlot {
        &self.voices[handle.handle.index as usize]
            .as_ref()
            .expect("voice input was fully preflighted")
    }

    fn voice_gain_q15(
        &self,
        voice: &VoiceSlot,
        inputs: &[AudioMixInput<'_>],
        solo_active: bool,
    ) -> u16 {
        if !self.bus_audible(voice.bus, solo_active) {
            return 0;
        }
        let mut gain = voice.gain_q15;
        let mut cursor = Some(voice.bus);
        while let Some(bus_ref) = cursor {
            let Some(bus) = self.buses.get(&bus_ref) else {
                return 0;
            };
            if bus.mute {
                return 0;
            }
            gain = multiply_q15(gain, bus.gain_q15);
            cursor = bus.parent;
        }
        for rule in &self.ducking {
            if self.is_descendant_or_same(voice.bus, rule.target)
                && self.trigger_active(rule.trigger, inputs, solo_active)
            {
                gain = multiply_q15(gain, rule.gain_q15);
            }
        }
        gain
    }

    fn trigger_active(
        &self,
        trigger: StableControlRef,
        inputs: &[AudioMixInput<'_>],
        solo_active: bool,
    ) -> bool {
        inputs.iter().any(|input| {
            let Some(voice) = self.voices[input.voice.handle.index as usize].as_ref() else {
                return false;
            };
            self.is_descendant_or_same(voice.bus, trigger)
                && self.bus_audible(voice.bus, solo_active)
        })
    }

    fn bus_audible(&self, bus: StableControlRef, solo_active: bool) -> bool {
        if !solo_active {
            return true;
        }
        let mut cursor = Some(bus);
        while let Some(bus_ref) = cursor {
            let Some(descriptor) = self.buses.get(&bus_ref) else {
                return false;
            };
            if descriptor.solo {
                return true;
            }
            cursor = descriptor.parent;
        }
        false
    }

    fn is_descendant_or_same(&self, mut bus: StableControlRef, ancestor: StableControlRef) -> bool {
        loop {
            if bus == ancestor {
                return true;
            }
            let Some(parent) = self
                .buses
                .get(&bus)
                .and_then(|descriptor| descriptor.parent)
            else {
                return false;
            };
            bus = parent;
        }
    }
}

fn validate_control_ref(
    engine: EngineId,
    reference: StableControlRef,
    kind: ControlKind,
) -> Result<(), AudioMixerError> {
    if reference.engine != engine {
        return Err(AudioMixerError::WrongEngine);
    }
    if reference.kind != kind || !reference.handle.is_valid() {
        return Err(AudioMixerError::InvalidBus);
    }
    Ok(())
}

fn validate_gain(gain: u16) -> Result<(), AudioMixerError> {
    if gain > UNITY_Q15 {
        return Err(AudioMixerError::InvalidGain);
    }
    Ok(())
}

fn validate_spatial(spatial: AudioSpatialSource) -> Result<(), AudioMixerError> {
    if spatial.max_distance_mm == 0 || spatial.min_distance_mm > spatial.max_distance_mm {
        return Err(AudioMixerError::InvalidSpatial);
    }
    Ok(())
}

fn validate_listener(listener: AudioListener) -> Result<(), AudioMixerError> {
    if listener.right_q15 == [0; 3]
        || listener
            .right_q15
            .iter()
            .any(|component| *component == i16::MIN)
    {
        return Err(AudioMixerError::InvalidListener);
    }
    Ok(())
}

fn validate_bus_acyclic(
    start: StableControlRef,
    buses: &BTreeMap<StableControlRef, AudioBusDescriptor>,
) -> Result<(), AudioMixerError> {
    let mut visited = BTreeSet::new();
    let mut cursor = Some(start);
    while let Some(bus) = cursor {
        if !visited.insert(bus) {
            return Err(AudioMixerError::BusCycle);
        }
        cursor = buses.get(&bus).and_then(|descriptor| descriptor.parent);
    }
    Ok(())
}

fn multiply_q15(left: u16, right: u16) -> u16 {
    (((left as u32 * right as u32) + (UNITY_Q15 as u32 / 2)) / UNITY_Q15 as u32) as u16
}

fn apply_gain(sample: i64, gain: u16) -> i64 {
    sample * gain as i64 / UNITY_Q15 as i64
}

fn spatial_gains(listener: AudioListener, source: AudioSpatialSource) -> (u16, u16, u16) {
    let dx = source.position_mm[0] as i64 - listener.position_mm[0] as i64;
    let dy = source.position_mm[1] as i64 - listener.position_mm[1] as i64;
    let dz = source.position_mm[2] as i64 - listener.position_mm[2] as i64;
    let squared = dx.unsigned_abs() as u128 * dx.unsigned_abs() as u128
        + dy.unsigned_abs() as u128 * dy.unsigned_abs() as u128
        + dz.unsigned_abs() as u128 * dz.unsigned_abs() as u128;
    let distance = integer_sqrt(squared).min(u64::MAX as u128) as u64;
    let min_distance = source.min_distance_mm as u64;
    let max_distance = source.max_distance_mm as u64;
    let attenuation = if distance <= min_distance {
        UNITY_Q15
    } else if distance >= max_distance {
        0
    } else {
        let remaining = max_distance - distance;
        let range = max_distance - min_distance;
        ((remaining * UNITY_Q15 as u64) / range) as u16
    };
    let pan = if distance == 0 {
        0_i64
    } else {
        let dot = dx * listener.right_q15[0] as i64
            + dy * listener.right_q15[1] as i64
            + dz * listener.right_q15[2] as i64;
        (dot / distance as i64).clamp(-(UNITY_Q15 as i64), UNITY_Q15 as i64)
    };
    let left = ((UNITY_Q15 as i64 - pan) / 2) as u16;
    let right = ((UNITY_Q15 as i64 + pan) / 2) as u16;
    (attenuation, left, right)
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut low = 1_u128;
    let mut high = value.min(u64::MAX as u128) + 1;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if middle <= value / middle {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

fn clamp_i16(value: i64) -> (i16, bool) {
    if value > i16::MAX as i64 {
        (i16::MAX, true)
    } else if value < i16::MIN as i64 {
        (i16::MIN, true)
    } else {
        (value as i16, false)
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

    fn control(kind: ControlKind, index: u32) -> StableControlRef {
        StableControlRef {
            engine: handle(1),
            kind,
            handle: handle(index),
        }
    }

    fn bus(index: u32) -> StableControlRef {
        control(ControlKind::AudioBus, index)
    }

    fn source(index: u32) -> StableControlRef {
        control(ControlKind::PersistentAudioSource, index)
    }

    fn bus_descriptor(index: u32, parent: Option<u32>, gain_q15: u16) -> AudioBusDescriptor {
        AudioBusDescriptor {
            bus: bus(index),
            parent: parent.map(bus),
            gain_q15,
            mute: false,
            solo: false,
        }
    }

    fn state(revision: u64) -> AudioControlState {
        AudioControlState {
            engine: handle(1),
            revision,
            buses: vec![
                bus_descriptor(1, None, UNITY_Q15),
                bus_descriptor(2, Some(1), UNITY_Q15),
                bus_descriptor(3, Some(1), UNITY_Q15),
            ],
            sources: vec![PersistentAudioSourceDescriptor {
                source: source(10),
                bus: bus(2),
                asset: crate::asset::AssetRef {
                    engine: handle(1),
                    handle: handle(20),
                },
                gain_q15: UNITY_Q15,
                spatial: None,
                looped: true,
                recovery: crate::audio::PersistentSourceRecoveryPolicy::ResumeTimeline,
                transport_anchor_millis: 0,
            }],
            ducking: vec![AudioDuckingRule {
                trigger: bus(3),
                target: bus(2),
                gain_q15: (UNITY_Q15 + 1) / 2,
            }],
            listener: AudioListener::default(),
        }
    }

    fn endpoint() -> AudioMixerEndpoint {
        AudioMixerEndpoint::new(
            handle(1),
            handle(50),
            AudioMixerConfig {
                max_buses: 4,
                max_sources: 4,
                max_ducking_rules: 4,
                max_voices: 4,
                max_callback_frames: 4,
            },
        )
        .expect("endpoint")
    }

    #[test]
    fn hierarchy_ducking_and_solo_mix_in_one_atomic_callback() {
        let mut endpoint = endpoint();
        endpoint.apply_control_state(state(1)).expect("control");
        let music = endpoint.start_persistent(source(10)).expect("music");
        let sfx = endpoint
            .start_one_shot(AudioOneShotDescriptor {
                event_id: 7,
                asset: AssetRef {
                    engine: handle(1),
                    handle: handle(21),
                },
                bus: bus(3),
                gain_q15: UNITY_Q15,
                spatial: None,
            })
            .expect("sfx");
        let music_samples = [1000_i16, 1000];
        let sfx_samples = [1000_i16, 1000];
        let inputs = [
            AudioMixInput {
                voice: music,
                channels: 1,
                samples: &music_samples,
            },
            AudioMixInput {
                voice: sfx,
                channels: 1,
                samples: &sfx_samples,
            },
        ];
        let mut output = [0_i16; 4];
        let report = endpoint.mix_stereo_into(&inputs, &mut output).expect("mix");
        assert_eq!(output, [1500, 1500, 1500, 1500]);
        assert_eq!(report.audible_voices, 2);

        let mut solo = state(2);
        solo.buses[1].solo = true;
        endpoint.apply_control_state(solo).expect("solo control");
        endpoint
            .mix_stereo_into(&inputs, &mut output)
            .expect("solo mix");
        assert_eq!(output, [1000, 1000, 1000, 1000]);
    }

    #[test]
    fn spatial_source_uses_listener_distance_and_right_axis() {
        let mut endpoint = endpoint();
        let mut control = state(1);
        control.sources[0].spatial = Some(AudioSpatialSource {
            position_mm: [500, 0, 0],
            min_distance_mm: 0,
            max_distance_mm: 1000,
        });
        endpoint.apply_control_state(control).expect("control");
        let voice = endpoint.start_persistent(source(10)).expect("voice");
        let samples = [1000_i16];
        let inputs = [AudioMixInput {
            voice,
            channels: 1,
            samples: &samples,
        }];
        let mut output = [0_i16; 2];
        endpoint.mix_stereo_into(&inputs, &mut output).expect("mix");
        assert_eq!(output[0], 0);
        assert!((499..=500).contains(&output[1]));
        assert_eq!(endpoint.voice_transport_frames(voice), Ok(1));
    }

    #[test]
    fn invalid_control_and_input_leave_state_output_and_transport_unchanged() {
        let mut endpoint = endpoint();
        endpoint.apply_control_state(state(1)).expect("control");
        let voice = endpoint.start_persistent(source(10)).expect("voice");
        let mut cyclic = state(2);
        cyclic.buses[0].parent = Some(bus(2));
        assert_eq!(
            endpoint.apply_control_state(cyclic),
            Err(AudioMixerError::BusCycle)
        );
        assert_eq!(endpoint.control_revision(), 1);

        let samples = [1_i16, 2];
        let duplicate = [
            AudioMixInput {
                voice,
                channels: 1,
                samples: &samples,
            },
            AudioMixInput {
                voice,
                channels: 1,
                samples: &samples,
            },
        ];
        let mut output = [77_i16; 4];
        assert_eq!(
            endpoint.mix_stereo_into(&duplicate, &mut output),
            Err(AudioMixerError::DuplicateVoiceInput)
        );
        assert_eq!(output, [77; 4]);
        assert_eq!(endpoint.voice_transport_frames(voice), Ok(0));
    }

    #[test]
    fn voice_generation_and_endpoint_restart_reject_late_handles() {
        let mut endpoint = endpoint();
        endpoint.apply_control_state(state(1)).expect("control");
        let first = endpoint.start_persistent(source(10)).expect("first");
        endpoint.stop_voice(first).expect("stop");
        let second = endpoint.start_persistent(source(10)).expect("second");
        assert_eq!(first.handle.index, second.handle.index);
        assert_eq!(first.handle.generation + 1, second.handle.generation);
        assert_eq!(
            endpoint.voice_transport_frames(first),
            Err(AudioMixerError::UnknownVoice)
        );
        endpoint.restart().expect("restart");
        assert_eq!(
            endpoint.voice_transport_frames(second),
            Err(AudioMixerError::StaleEndpoint)
        );
        assert_eq!(endpoint.control_revision(), 1);
    }
}
