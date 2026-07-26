use std::collections::{BTreeSet, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::audio::{PersistentSourceRecoveryAction, PersistentSourceRecoveryPolicy};
use crate::audio_mixer::PersistentAudioSourceDescriptor;
use crate::control::{ControlKind, StableControlRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioStreamingConfig {
    pub max_streams: usize,
    pub max_total_ring_samples: usize,
    pub max_channels: u16,
    pub max_decode_frames: usize,
    pub max_callback_frames: usize,
}

impl Default for AudioStreamingConfig {
    fn default() -> Self {
        Self {
            max_streams: 64,
            max_total_ring_samples: 4 * 1024 * 1024,
            max_channels: 8,
            max_decode_frames: 16_384,
            max_callback_frames: 4096,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioStreamDescriptor {
    pub source: StableControlRef,
    pub sample_rate: u32,
    pub channels: u16,
    pub ring_capacity_frames: usize,
    pub looped: bool,
    pub recovery: PersistentSourceRecoveryPolicy,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AudioStreamHandle {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDecodeWork {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub decode_actor: Handle,
    pub stream: AudioStreamHandle,
    pub request_sequence: u64,
    pub start_frame: u64,
    pub max_frames: usize,
    pub channels: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioDecodeCompletion {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub decode_actor: Handle,
    pub stream: AudioStreamHandle,
    pub request_sequence: u64,
    pub start_frame: u64,
    pub channels: u16,
    pub samples: Vec<i16>,
    pub end_of_stream: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioRenderReport {
    pub stream: AudioStreamHandle,
    pub requested_frames: usize,
    pub rendered_frames: usize,
    pub silence_frames: usize,
    pub transport_frame: u64,
    pub buffered_frames: usize,
    pub end_of_stream: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioStreamRecoveryPlan {
    pub engine: EngineId,
    pub source: StableControlRef,
    pub descriptor: AudioStreamDescriptor,
    pub action: PersistentSourceRecoveryAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioStreamingOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub closed: bool,
    pub live_streams: usize,
    pub pending_decodes: usize,
    pub reserved_ring_samples: usize,
    pub buffered_samples: usize,
    pub underflow_frames: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioStreamingShutdownReport {
    pub endpoint_generation: Handle,
    pub released_streams: usize,
    pub released_ring_samples: usize,
    pub released_buffered_samples: usize,
    pub cancelled_decodes: Vec<AudioDecodeWork>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStreamingError {
    InvalidConfig,
    WrongEngine,
    StaleEndpoint,
    WrongDecodeActor,
    InvalidSource,
    DuplicateSource,
    InvalidDescriptor,
    StreamCapacity,
    RingCapacity,
    UnknownStream,
    GenerationExhausted,
    DecodeAlreadyPending,
    DecodeUnavailable,
    UnknownDecodeWork,
    DecodeShape,
    DecodeStartMismatch,
    DecodeCapacity,
    CallbackShape,
    CallbackCapacity,
    ArithmeticOverflow,
    RequestSequenceExhausted,
    EndpointGenerationExhausted,
    RecoveryAction,
    Closed,
}

#[derive(Debug)]
struct AudioStreamSlot {
    descriptor: AudioStreamDescriptor,
    ring: VecDeque<i16>,
    ring_capacity_samples: usize,
    decoded_until_frame: u64,
    transport_frame: u64,
    pending: Option<AudioDecodeWork>,
    end_of_stream: bool,
    underflow_frames: u64,
}

pub struct AudioStreamingEndpoint {
    engine: EngineId,
    endpoint_generation: Handle,
    decode_actor: Handle,
    config: AudioStreamingConfig,
    slots: Vec<Option<AudioStreamSlot>>,
    generations: Vec<u32>,
    free: BTreeSet<u32>,
    total_ring_samples: usize,
    next_request_sequence: u64,
    closed: bool,
}

impl AudioStreamingEndpoint {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        decode_actor: Handle,
        config: AudioStreamingConfig,
    ) -> Result<Self, AudioStreamingError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || !decode_actor.is_valid()
            || config.max_streams == 0
            || config.max_total_ring_samples == 0
            || config.max_channels == 0
            || config.max_decode_frames == 0
            || config.max_callback_frames == 0
        {
            return Err(AudioStreamingError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            decode_actor,
            config,
            slots: Vec::new(),
            generations: Vec::new(),
            free: BTreeSet::new(),
            total_ring_samples: 0,
            next_request_sequence: 0,
            closed: false,
        })
    }

    pub const fn endpoint_generation(&self) -> Handle {
        self.endpoint_generation
    }

    pub const fn total_ring_samples(&self) -> usize {
        self.total_ring_samples
    }

    pub fn owner_snapshot(&self) -> AudioStreamingOwnerSnapshot {
        AudioStreamingOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            closed: self.closed,
            live_streams: self.slots.iter().flatten().count(),
            pending_decodes: self
                .slots
                .iter()
                .flatten()
                .filter(|slot| slot.pending.is_some())
                .count(),
            reserved_ring_samples: self.total_ring_samples,
            buffered_samples: self
                .slots
                .iter()
                .flatten()
                .map(|slot| slot.ring.len())
                .sum(),
            underflow_frames: self
                .slots
                .iter()
                .flatten()
                .map(|slot| slot.underflow_frames)
                .sum(),
        }
    }

    pub fn create_stream(
        &mut self,
        descriptor: AudioStreamDescriptor,
    ) -> Result<AudioStreamHandle, AudioStreamingError> {
        self.ensure_open()?;
        self.validate_descriptor(&descriptor)?;
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.descriptor.source == descriptor.source)
        {
            return Err(AudioStreamingError::DuplicateSource);
        }
        let ring_samples = ring_samples(&descriptor)?;
        let new_total = self
            .total_ring_samples
            .checked_add(ring_samples)
            .filter(|total| *total <= self.config.max_total_ring_samples)
            .ok_or(AudioStreamingError::RingCapacity)?;

        let index = if let Some(index) = self.free.first().copied() {
            index
        } else {
            if self.slots.len() == self.config.max_streams || self.slots.len() >= u32::MAX as usize
            {
                return Err(AudioStreamingError::StreamCapacity);
            }
            self.slots.len() as u32
        };
        let index_usize = index as usize;
        let generation = if index_usize == self.generations.len() {
            self.generations.push(1);
            self.slots.push(None);
            1
        } else {
            self.generations[index_usize]
        };
        self.free.remove(&index);
        self.slots[index_usize] = Some(AudioStreamSlot {
            descriptor,
            ring: VecDeque::with_capacity(ring_samples),
            ring_capacity_samples: ring_samples,
            decoded_until_frame: 0,
            transport_frame: 0,
            pending: None,
            end_of_stream: false,
            underflow_frames: 0,
        });
        self.total_ring_samples = new_total;
        Ok(AudioStreamHandle {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle: Handle { index, generation },
        })
    }

    pub fn create_stream_from_control(
        &mut self,
        control: &PersistentAudioSourceDescriptor,
        sample_rate: u32,
        channels: u16,
        ring_capacity_frames: usize,
    ) -> Result<AudioStreamHandle, AudioStreamingError> {
        if control.source.engine != self.engine || control.asset.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        let start_frame = frames_from_millis(control.transport_anchor_millis, sample_rate)?;
        let stream = self.create_stream(AudioStreamDescriptor {
            source: control.source,
            sample_rate,
            channels,
            ring_capacity_frames,
            looped: control.looped,
            recovery: control.recovery,
        })?;
        let index = self.validate_stream_index(stream)?;
        let slot = self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?;
        slot.decoded_until_frame = start_frame;
        slot.transport_frame = start_frame;
        Ok(stream)
    }

    pub fn destroy_stream(&mut self, stream: AudioStreamHandle) -> Result<(), AudioStreamingError> {
        self.ensure_open()?;
        let index = self.validate_stream_index(stream)?;
        let ring_samples = {
            let slot = self.slots[index]
                .as_ref()
                .ok_or(AudioStreamingError::UnknownStream)?;
            ring_samples(&slot.descriptor)?
        };
        let next_generation = self.generations[index]
            .checked_add(1)
            .ok_or(AudioStreamingError::GenerationExhausted)?;
        self.generations[index] = next_generation;
        self.slots[index] = None;
        self.free.insert(index as u32);
        self.total_ring_samples -= ring_samples;
        Ok(())
    }

    pub fn request_decode(
        &mut self,
        stream: AudioStreamHandle,
    ) -> Result<AudioDecodeWork, AudioStreamingError> {
        self.ensure_open()?;
        let index = self.validate_stream_index(stream)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?;
        if slot.pending.is_some() {
            return Err(AudioStreamingError::DecodeAlreadyPending);
        }
        if slot.end_of_stream && !slot.descriptor.looped {
            return Err(AudioStreamingError::DecodeUnavailable);
        }
        let channels = slot.descriptor.channels as usize;
        let free_samples = slot.ring_capacity_samples.saturating_sub(slot.ring.len());
        let free_frames = free_samples / channels;
        let max_frames = free_frames.min(self.config.max_decode_frames);
        if max_frames == 0 {
            return Err(AudioStreamingError::DecodeUnavailable);
        }
        let request_sequence = self
            .next_request_sequence
            .checked_add(1)
            .ok_or(AudioStreamingError::RequestSequenceExhausted)?;
        let start_frame = if slot.end_of_stream && slot.descriptor.looped {
            0
        } else {
            slot.decoded_until_frame
        };
        let work = AudioDecodeWork {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            decode_actor: self.decode_actor,
            stream,
            request_sequence,
            start_frame,
            max_frames,
            channels: slot.descriptor.channels,
        };
        let slot = self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?;
        slot.pending = Some(work);
        slot.decoded_until_frame = start_frame;
        slot.end_of_stream = false;
        self.next_request_sequence = request_sequence;
        Ok(work)
    }

    pub fn complete_decode(
        &mut self,
        completion: AudioDecodeCompletion,
    ) -> Result<usize, AudioStreamingError> {
        self.ensure_open()?;
        self.validate_completion_owner(&completion)?;
        let index = self.validate_stream_index(completion.stream)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?;
        let pending = slot.pending.ok_or(AudioStreamingError::UnknownDecodeWork)?;
        if pending.request_sequence != completion.request_sequence
            || pending.stream != completion.stream
        {
            return Err(AudioStreamingError::UnknownDecodeWork);
        }
        if completion.start_frame != pending.start_frame
            || completion.start_frame != slot.decoded_until_frame
        {
            return Err(AudioStreamingError::DecodeStartMismatch);
        }
        if completion.channels != pending.channels || completion.channels == 0 {
            return Err(AudioStreamingError::DecodeShape);
        }
        let channels = completion.channels as usize;
        if completion.samples.len() % channels != 0 {
            return Err(AudioStreamingError::DecodeShape);
        }
        let frames = completion.samples.len() / channels;
        if frames > pending.max_frames || (frames == 0 && !completion.end_of_stream) {
            return Err(AudioStreamingError::DecodeCapacity);
        }
        let new_decoded_until = slot
            .decoded_until_frame
            .checked_add(frames as u64)
            .ok_or(AudioStreamingError::ArithmeticOverflow)?;
        if completion.samples.len() > slot.ring_capacity_samples.saturating_sub(slot.ring.len()) {
            return Err(AudioStreamingError::DecodeCapacity);
        }

        let slot = self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?;
        slot.ring.extend(completion.samples);
        slot.decoded_until_frame = new_decoded_until;
        slot.end_of_stream = completion.end_of_stream;
        slot.pending = None;
        Ok(frames)
    }

    pub fn fail_decode(&mut self, work: AudioDecodeWork) -> Result<(), AudioStreamingError> {
        self.ensure_open()?;
        self.validate_work_owner(work)?;
        let index = self.validate_stream_index(work.stream)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?;
        if slot.pending != Some(work) {
            return Err(AudioStreamingError::UnknownDecodeWork);
        }
        self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?
            .pending = None;
        Ok(())
    }

    pub fn render_into(
        &mut self,
        stream: AudioStreamHandle,
        output: &mut [i16],
    ) -> Result<AudioRenderReport, AudioStreamingError> {
        self.ensure_open()?;
        let index = self.validate_stream_index(stream)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?;
        let channels = slot.descriptor.channels as usize;
        if output.len() % channels != 0 {
            return Err(AudioStreamingError::CallbackShape);
        }
        let requested_frames = output.len() / channels;
        if requested_frames > self.config.max_callback_frames {
            return Err(AudioStreamingError::CallbackCapacity);
        }
        let available_frames = slot.ring.len() / channels;
        let rendered_frames = available_frames.min(requested_frames);
        let silence_frames = requested_frames - rendered_frames;
        let new_transport = slot
            .transport_frame
            .checked_add(requested_frames as u64)
            .ok_or(AudioStreamingError::ArithmeticOverflow)?;
        let new_underflow = slot
            .underflow_frames
            .checked_add(silence_frames as u64)
            .ok_or(AudioStreamingError::ArithmeticOverflow)?;

        let slot = self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?;
        let rendered_samples = rendered_frames * channels;
        for sample in output.iter_mut().take(rendered_samples) {
            *sample = slot
                .ring
                .pop_front()
                .ok_or(AudioStreamingError::ArithmeticOverflow)?;
        }
        output[rendered_samples..].fill(0);
        slot.transport_frame = new_transport;
        slot.underflow_frames = new_underflow;
        Ok(AudioRenderReport {
            stream,
            requested_frames,
            rendered_frames,
            silence_frames,
            transport_frame: new_transport,
            buffered_frames: slot.ring.len() / channels,
            end_of_stream: slot.end_of_stream && slot.ring.is_empty(),
        })
    }

    pub fn buffered_frames(&self, stream: AudioStreamHandle) -> Result<usize, AudioStreamingError> {
        self.ensure_open()?;
        let index = self.validate_stream_index(stream)?;
        let slot = self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?;
        Ok(slot.ring.len() / slot.descriptor.channels as usize)
    }

    pub fn underflow_frames(&self, stream: AudioStreamHandle) -> Result<u64, AudioStreamingError> {
        self.ensure_open()?;
        let index = self.validate_stream_index(stream)?;
        Ok(self.slots[index]
            .as_ref()
            .ok_or(AudioStreamingError::UnknownStream)?
            .underflow_frames)
    }

    pub fn restart(&mut self) -> Result<Vec<AudioStreamRecoveryPlan>, AudioStreamingError> {
        self.ensure_open()?;
        let next_generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioStreamingError::EndpointGenerationExhausted)?;
        let mut plans = Vec::with_capacity(self.slots.iter().flatten().count());
        for slot in self.slots.iter().flatten() {
            let millis = transport_millis(slot.transport_frame, slot.descriptor.sample_rate)?;
            plans.push(AudioStreamRecoveryPlan {
                engine: self.engine,
                source: slot.descriptor.source,
                descriptor: slot.descriptor.clone(),
                action: slot.descriptor.recovery.action(millis),
            });
        }
        plans.sort_by_key(|plan| plan.source);
        self.endpoint_generation.generation = next_generation;
        self.slots.clear();
        self.generations.clear();
        self.free.clear();
        self.total_ring_samples = 0;
        Ok(plans)
    }

    pub fn restore_stream(
        &mut self,
        plan: AudioStreamRecoveryPlan,
    ) -> Result<Option<AudioStreamHandle>, AudioStreamingError> {
        self.ensure_open()?;
        if plan.engine != self.engine || plan.source.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        if plan.source != plan.descriptor.source {
            return Err(AudioStreamingError::InvalidSource);
        }
        let start_frame = match plan.action {
            PersistentSourceRecoveryAction::ResumeAtMillis(millis) => {
                frames_from_millis(millis, plan.descriptor.sample_rate)?
            }
            PersistentSourceRecoveryAction::RestartAtZero => 0,
            PersistentSourceRecoveryAction::Stop => return Ok(None),
        };
        let stream = self.create_stream(plan.descriptor)?;
        let index = self.validate_stream_index(stream)?;
        let slot = self.slots[index]
            .as_mut()
            .ok_or(AudioStreamingError::UnknownStream)?;
        slot.decoded_until_frame = start_frame;
        slot.transport_frame = start_frame;
        Ok(Some(stream))
    }

    pub fn shutdown(&mut self) -> Result<AudioStreamingShutdownReport, AudioStreamingError> {
        if self.closed {
            return Ok(AudioStreamingShutdownReport {
                endpoint_generation: self.endpoint_generation,
                released_streams: 0,
                released_ring_samples: 0,
                released_buffered_samples: 0,
                cancelled_decodes: Vec::new(),
            });
        }
        self.preflight_shutdown()?;
        let generation = self.endpoint_generation.generation + 1;
        let cancelled_decodes = self
            .slots
            .iter()
            .flatten()
            .filter_map(|slot| slot.pending)
            .collect();
        let report = AudioStreamingShutdownReport {
            endpoint_generation: Handle {
                index: self.endpoint_generation.index,
                generation,
            },
            released_streams: self.slots.iter().flatten().count(),
            released_ring_samples: self.total_ring_samples,
            released_buffered_samples: self
                .slots
                .iter()
                .flatten()
                .map(|slot| slot.ring.len())
                .sum(),
            cancelled_decodes,
        };
        self.endpoint_generation.generation = generation;
        self.slots.clear();
        self.generations.clear();
        self.free.clear();
        self.total_ring_samples = 0;
        self.closed = true;
        Ok(report)
    }

    pub fn preflight_shutdown(&self) -> Result<(), AudioStreamingError> {
        if self.closed {
            return Ok(());
        }
        self.endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioStreamingError::EndpointGenerationExhausted)?;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), AudioStreamingError> {
        if self.closed {
            Err(AudioStreamingError::Closed)
        } else {
            Ok(())
        }
    }

    fn validate_descriptor(
        &self,
        descriptor: &AudioStreamDescriptor,
    ) -> Result<(), AudioStreamingError> {
        if descriptor.source.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        if descriptor.source.kind != ControlKind::PersistentAudioSource
            || !descriptor.source.handle.is_valid()
        {
            return Err(AudioStreamingError::InvalidSource);
        }
        if descriptor.sample_rate == 0
            || descriptor.channels == 0
            || descriptor.channels > self.config.max_channels
            || descriptor.ring_capacity_frames == 0
        {
            return Err(AudioStreamingError::InvalidDescriptor);
        }
        ring_samples(descriptor)?;
        Ok(())
    }

    fn validate_stream_index(
        &self,
        stream: AudioStreamHandle,
    ) -> Result<usize, AudioStreamingError> {
        if stream.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        if stream.endpoint_generation != self.endpoint_generation {
            return Err(AudioStreamingError::StaleEndpoint);
        }
        if !stream.handle.is_valid() {
            return Err(AudioStreamingError::UnknownStream);
        }
        let index = stream.handle.index as usize;
        if self.generations.get(index).copied() != Some(stream.handle.generation)
            || self.slots.get(index).and_then(Option::as_ref).is_none()
        {
            return Err(AudioStreamingError::UnknownStream);
        }
        Ok(index)
    }

    fn validate_work_owner(&self, work: AudioDecodeWork) -> Result<(), AudioStreamingError> {
        if work.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        if work.endpoint_generation != self.endpoint_generation {
            return Err(AudioStreamingError::StaleEndpoint);
        }
        if work.decode_actor != self.decode_actor {
            return Err(AudioStreamingError::WrongDecodeActor);
        }
        Ok(())
    }

    fn validate_completion_owner(
        &self,
        completion: &AudioDecodeCompletion,
    ) -> Result<(), AudioStreamingError> {
        if completion.engine != self.engine {
            return Err(AudioStreamingError::WrongEngine);
        }
        if completion.endpoint_generation != self.endpoint_generation {
            return Err(AudioStreamingError::StaleEndpoint);
        }
        if completion.decode_actor != self.decode_actor {
            return Err(AudioStreamingError::WrongDecodeActor);
        }
        Ok(())
    }
}

fn ring_samples(descriptor: &AudioStreamDescriptor) -> Result<usize, AudioStreamingError> {
    descriptor
        .ring_capacity_frames
        .checked_mul(descriptor.channels as usize)
        .ok_or(AudioStreamingError::ArithmeticOverflow)
}

fn transport_millis(frames: u64, sample_rate: u32) -> Result<u64, AudioStreamingError> {
    frames
        .checked_mul(1000)
        .map(|value| value / sample_rate as u64)
        .ok_or(AudioStreamingError::ArithmeticOverflow)
}

fn frames_from_millis(millis: u64, sample_rate: u32) -> Result<u64, AudioStreamingError> {
    millis
        .checked_mul(sample_rate as u64)
        .map(|value| value / 1000)
        .ok_or(AudioStreamingError::ArithmeticOverflow)
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

    fn source(index: u32) -> StableControlRef {
        StableControlRef {
            engine: handle(1),
            kind: ControlKind::PersistentAudioSource,
            handle: handle(index),
        }
    }

    fn descriptor(index: u32, recovery: PersistentSourceRecoveryPolicy) -> AudioStreamDescriptor {
        AudioStreamDescriptor {
            source: source(index),
            sample_rate: 1000,
            channels: 2,
            ring_capacity_frames: 4,
            looped: false,
            recovery,
        }
    }

    fn endpoint() -> AudioStreamingEndpoint {
        AudioStreamingEndpoint::new(
            handle(1),
            handle(10),
            handle(20),
            AudioStreamingConfig {
                max_streams: 3,
                max_total_ring_samples: 24,
                max_channels: 2,
                max_decode_frames: 3,
                max_callback_frames: 4,
            },
        )
        .expect("valid endpoint")
    }

    fn completion(work: AudioDecodeWork, samples: Vec<i16>) -> AudioDecodeCompletion {
        AudioDecodeCompletion {
            engine: work.engine,
            endpoint_generation: work.endpoint_generation,
            decode_actor: work.decode_actor,
            stream: work.stream,
            request_sequence: work.request_sequence,
            start_frame: work.start_frame,
            channels: work.channels,
            samples,
            end_of_stream: false,
        }
    }

    #[test]
    fn decode_ring_and_callback_are_bounded_and_fill_underflow_with_silence() {
        let mut endpoint = endpoint();
        let stream = endpoint
            .create_stream(descriptor(
                30,
                PersistentSourceRecoveryPolicy::ResumeTimeline,
            ))
            .expect("stream");
        let work = endpoint.request_decode(stream).expect("work");
        assert_eq!(work.max_frames, 3);
        assert_eq!(
            endpoint.complete_decode(completion(work, vec![1, 2, 3, 4, 5, 6])),
            Ok(3)
        );
        assert_eq!(endpoint.buffered_frames(stream), Ok(3));

        let mut output = [99_i16; 8];
        let report = endpoint.render_into(stream, &mut output).expect("render");
        assert_eq!(output, [1, 2, 3, 4, 5, 6, 0, 0]);
        assert_eq!(report.rendered_frames, 3);
        assert_eq!(report.silence_frames, 1);
        assert_eq!(report.transport_frame, 4);
        assert_eq!(endpoint.underflow_frames(stream), Ok(1));
        assert_eq!(endpoint.total_ring_samples(), 8);
    }

    #[test]
    fn owner_shape_capacity_and_generation_fail_before_pcm_exposure() {
        let mut endpoint = endpoint();
        let stream = endpoint
            .create_stream(descriptor(
                30,
                PersistentSourceRecoveryPolicy::ResumeTimeline,
            ))
            .expect("stream");
        assert_eq!(
            endpoint.create_stream(descriptor(30, PersistentSourceRecoveryPolicy::Restart)),
            Err(AudioStreamingError::DuplicateSource)
        );
        let work = endpoint.request_decode(stream).expect("work");
        let mut foreign = completion(work, vec![1, 2]);
        foreign.decode_actor = handle(21);
        assert_eq!(
            endpoint.complete_decode(foreign),
            Err(AudioStreamingError::WrongDecodeActor)
        );
        assert_eq!(endpoint.buffered_frames(stream), Ok(0));
        let malformed = completion(work, vec![1, 2, 3]);
        assert_eq!(
            endpoint.complete_decode(malformed),
            Err(AudioStreamingError::DecodeShape)
        );
        assert_eq!(endpoint.buffered_frames(stream), Ok(0));
        endpoint
            .complete_decode(completion(work, vec![1, 2]))
            .expect("valid completion");
        endpoint.destroy_stream(stream).expect("destroy");
        assert_eq!(
            endpoint.buffered_frames(stream),
            Err(AudioStreamingError::UnknownStream)
        );
        let replacement = endpoint
            .create_stream(descriptor(31, PersistentSourceRecoveryPolicy::Restart))
            .expect("replacement");
        assert_eq!(replacement.handle.index, stream.handle.index);
        assert_eq!(replacement.handle.generation, stream.handle.generation + 1);
    }

    #[test]
    fn restart_plans_are_stable_and_restore_exact_declared_policy() {
        let mut endpoint = endpoint();
        let resume = endpoint
            .create_stream(descriptor(
                32,
                PersistentSourceRecoveryPolicy::ResumeTimeline,
            ))
            .expect("resume");
        endpoint
            .create_stream(descriptor(30, PersistentSourceRecoveryPolicy::Restart))
            .expect("restart");
        endpoint
            .create_stream(descriptor(
                31,
                PersistentSourceRecoveryPolicy::StopOnRecovery,
            ))
            .expect("stop");
        let work = endpoint.request_decode(resume).expect("work");
        endpoint
            .complete_decode(completion(work, vec![1, 2, 3, 4, 5, 6]))
            .expect("completion");
        let mut output = [0_i16; 4];
        endpoint.render_into(resume, &mut output).expect("render");

        let old_generation = endpoint.endpoint_generation();
        let plans = endpoint.restart().expect("restart endpoint");
        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].source, source(30));
        assert_eq!(plans[1].source, source(31));
        assert_eq!(plans[2].source, source(32));
        assert_eq!(
            plans[0].action,
            PersistentSourceRecoveryAction::RestartAtZero
        );
        assert_eq!(plans[1].action, PersistentSourceRecoveryAction::Stop);
        assert_eq!(
            plans[2].action,
            PersistentSourceRecoveryAction::ResumeAtMillis(2)
        );
        assert_eq!(endpoint.endpoint_generation().generation, 2);
        assert_eq!(endpoint.total_ring_samples(), 0);
        assert_eq!(
            endpoint.buffered_frames(resume),
            Err(AudioStreamingError::StaleEndpoint)
        );

        let restart_stream = endpoint
            .restore_stream(plans[0].clone())
            .expect("restore restart")
            .expect("running restart");
        let resume_stream = endpoint
            .restore_stream(plans[2].clone())
            .expect("restore resume")
            .expect("running resume");
        assert_eq!(endpoint.restore_stream(plans[1].clone()), Ok(None));
        assert_eq!(
            endpoint
                .request_decode(restart_stream)
                .expect("work")
                .start_frame,
            0
        );
        assert_eq!(
            endpoint
                .request_decode(resume_stream)
                .expect("work")
                .start_frame,
            2
        );
        assert_ne!(endpoint.endpoint_generation(), old_generation);
    }

    #[test]
    fn looped_stream_rewinds_decode_cursor_only_after_verified_eof() {
        let mut endpoint = endpoint();
        let mut looping = descriptor(30, PersistentSourceRecoveryPolicy::Restart);
        looping.looped = true;
        let stream = endpoint.create_stream(looping).expect("stream");
        let first = endpoint.request_decode(stream).expect("first work");
        let mut end = completion(first, vec![1, 2, 3, 4]);
        end.end_of_stream = true;
        endpoint.complete_decode(end).expect("verified eof");

        let second = endpoint.request_decode(stream).expect("rewound work");
        assert_eq!(second.start_frame, 0);
        assert_eq!(second.max_frames, 2);
        endpoint
            .complete_decode(completion(second, vec![5, 6, 7, 8]))
            .expect("next loop");
        let mut output = [0_i16; 8];
        endpoint.render_into(stream, &mut output).expect("render");
        assert_eq!(output, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
