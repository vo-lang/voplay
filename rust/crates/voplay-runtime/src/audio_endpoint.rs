use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use vo_app_protocol::{
    AudioDeviceFormat, AudioDevicePermit, AudioRealtimeEndpoint, GenerationalHandle,
};
use voplay_protocol::{generated::MessageKind, EngineId, Handle, PacketHeader};

#[cfg(feature = "inspection")]
use crate::fault_injection::{
    FaultInjectionConfig, FaultInjectionError, FaultInjectionMetrics, FaultInjector, FaultPoint,
    FaultRule, InjectedFault,
};
#[cfg(feature = "inspection")]
use crate::fault_wire::{
    apply_fault_wire_command, fault_wire_response_packet, is_fault_wire_command,
};
use crate::{
    asset::AssetRef,
    audio::{
        AudioEndpoint, AudioEndpointConfig, AudioEndpointShutdownReport, AudioEndpointState,
        AudioEvent, AudioEventOutcome, AudioGestureToken, AudioPoll,
    },
    audio_control::{decode_audio_control_snapshot, AudioControlDecodeConfig},
    audio_device_adapter::{AudioAssetStore, AudioAssetStoreConfig, AudioAssetStoreShutdownReport},
    audio_mixer::{
        AudioControlState, AudioMixInput, AudioMixReport, AudioMixerConfig, AudioMixerEndpoint,
        AudioMixerShutdownReport, AudioOneShotDescriptor, AudioSpatialSource, AudioVoiceHandle,
    },
    audio_pipeline::{
        AudioPipeline, AudioPipelineConfig, AudioPipelineInput, AudioPipelineRender,
        AudioPipelineShutdownReport,
    },
    audio_streaming::AudioStreamingError,
    control::{
        is_tombstone_update, ControlDomain, ControlKind, ControlSnapshotEntry,
        ControlSnapshotState, ControlStateSnapshot, RealizationOutcome, RetirementFence,
        RetirementProgress, StableControlRef,
    },
    control_authority_wire::{
        decode_control_observed_ack, encode_control_observed, encode_control_realization_results,
    },
    control_wire::decode_audio_control_snapshot_parts,
    endpoint::{
        DispatchOutcome, EndpointDriver, EndpointDriverError, EndpointQueueMetrics,
        OwnedEndpointPacket,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointDriverConfig {
    pub endpoint: AudioEndpointConfig,
    pub mixer: AudioMixerConfig,
    pub pipeline: AudioPipelineConfig,
    pub assets: AudioAssetStoreConfig,
    pub control_decode: AudioControlDecodeConfig,
    pub stream_ring_frames: usize,
    pub worker_render_frames: usize,
    pub max_decode_passes_per_wake: usize,
    pub max_output_packets: usize,
    pub max_output_bytes: usize,
}

impl Default for AudioEndpointDriverConfig {
    fn default() -> Self {
        Self {
            endpoint: AudioEndpointConfig::default(),
            mixer: AudioMixerConfig::default(),
            pipeline: AudioPipelineConfig::default(),
            assets: AudioAssetStoreConfig::default(),
            control_decode: AudioControlDecodeConfig::default(),
            stream_ring_frames: 16_384,
            worker_render_frames: 512,
            max_decode_passes_per_wake: 4,
            max_output_packets: 4096,
            max_output_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointDriverState {
    Created,
    Running,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointDriverOwnerSnapshot {
    pub state: AudioEndpointDriverState,
    pub endpoint_state: AudioEndpointState,
    pub device_configured: bool,
    pub control_revision: u64,
    pub queued_events: usize,
    pub queued_event_bytes: usize,
    pub buses: usize,
    pub persistent_sources: usize,
    pub active_voices: usize,
    pub bound_streams: usize,
    pub resident_assets: usize,
    pub resident_asset_bytes: usize,
    pub one_shots: usize,
    pub recovery_stopped_sources: usize,
    pub control_entries: usize,
    pub retiring_control_entries: usize,
    pub asset_revisions: usize,
    pub executor_closed: bool,
    pub owned_audio_closed: bool,
    pub output: EndpointQueueMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEndpointDriverShutdownReport {
    pub endpoint: AudioEndpointShutdownReport,
    pub pipeline: AudioPipelineShutdownReport,
    pub mixer: AudioMixerShutdownReport,
    pub assets: AudioAssetStoreShutdownReport,
    pub released_one_shots: usize,
    pub released_control_entries: usize,
    pub released_retiring_control_entries: usize,
    pub released_asset_revisions: usize,
    pub released_recovery_stops: usize,
}

pub trait AudioEventExecutor: Send {
    fn activate(&mut self, _permit: AudioDevicePermit) -> Result<(), ()> {
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn resume(&mut self, _permit: AudioDevicePermit) -> Result<(), ()> {
        Ok(())
    }

    fn device_lost(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn submit_pcm(&mut self, _samples: &[i16]) -> Result<(), ()> {
        Ok(())
    }

    fn execute(
        &mut self,
        event: &AudioEvent,
        mixer: &mut AudioMixerEndpoint,
    ) -> AudioEventExecution;

    fn close(&mut self, deadline_micros: u64) -> Result<(), ()>;
}

pub struct MixerAudioEventExecutor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEventExecution {
    pub outcome: AudioEventOutcome,
    pub voice: Option<AudioVoiceHandle>,
}

impl AudioEventExecutor for MixerAudioEventExecutor {
    fn execute(
        &mut self,
        event: &AudioEvent,
        mixer: &mut AudioMixerEndpoint,
    ) -> AudioEventExecution {
        let Some(descriptor) = decode_one_shot_event(event) else {
            return AudioEventExecution {
                outcome: AudioEventOutcome::Failed,
                voice: None,
            };
        };
        match mixer.start_one_shot(descriptor) {
            Ok(voice) => AudioEventExecution {
                outcome: AudioEventOutcome::Executed,
                voice: Some(voice),
            },
            Err(_) => AudioEventExecution {
                outcome: AudioEventOutcome::FailedControlUnavailable,
                voice: None,
            },
        }
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct OneShotPlayback {
    voice: AudioVoiceHandle,
    asset: AssetRef,
    channels: u16,
    cursor_frame: u64,
}

pub struct AudioEndpointDriver {
    engine: EngineId,
    endpoint: AudioEndpoint,
    mixer: AudioMixerEndpoint,
    control: Option<AudioControlState>,
    control_snapshot: Option<ControlStateSnapshot>,
    retiring_audio_entries: BTreeMap<StableControlRef, (RetirementFence, ControlSnapshotEntry)>,
    last_control_source: Option<PacketHeader>,
    last_observed_progress: Option<(u64, u64, RetirementProgress)>,
    observed_domain_frame: u64,
    observed_audio_event_sequence: u64,
    pipeline: AudioPipeline,
    assets: AudioAssetStore,
    asset_revisions: BTreeMap<AssetRef, u64>,
    one_shots: Vec<OneShotPlayback>,
    recovery_stopped: BTreeSet<StableControlRef>,
    device_format: Option<AudioDeviceFormat>,
    executor: Box<dyn AudioEventExecutor>,
    executor_closed: bool,
    owned_audio_closed: bool,
    config: AudioEndpointDriverConfig,
    state: AudioEndpointDriverState,
    output: VecDeque<OwnedEndpointPacket>,
    output_bytes: usize,
    output_peak_packets: usize,
    output_peak_bytes: usize,
    output_capacity_rejections: u64,
    #[cfg(feature = "inspection")]
    faults: FaultInjector,
}

impl AudioEndpointDriver {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: AudioEndpointDriverConfig,
        executor: Box<dyn AudioEventExecutor>,
    ) -> Result<Self, EndpointDriverError> {
        if config.max_output_packets == 0
            || config.max_output_bytes == 0
            || config.stream_ring_frames == 0
            || config.worker_render_frames == 0
            || config.worker_render_frames > config.pipeline.max_render_frames
            || config.max_decode_passes_per_wake == 0
        {
            return Err(EndpointDriverError::Rejected);
        }
        let decode_actor = endpoint_generation;
        #[cfg(feature = "inspection")]
        let faults = FaultInjector::new(FaultInjectionConfig::default())
            .map_err(|_| EndpointDriverError::Rejected)?;
        Ok(Self {
            engine,
            endpoint: AudioEndpoint::new(engine, endpoint_generation, config.endpoint)
                .map_err(|_| EndpointDriverError::Rejected)?,
            mixer: AudioMixerEndpoint::new(engine, endpoint_generation, config.mixer)
                .map_err(|_| EndpointDriverError::Rejected)?,
            control: None,
            control_snapshot: None,
            retiring_audio_entries: BTreeMap::new(),
            last_control_source: None,
            last_observed_progress: None,
            observed_domain_frame: 0,
            observed_audio_event_sequence: 0,
            pipeline: AudioPipeline::new(
                engine,
                endpoint_generation,
                decode_actor,
                config.pipeline,
            )
            .map_err(|_| EndpointDriverError::Rejected)?,
            assets: AudioAssetStore::new(engine, decode_actor, config.assets)
                .map_err(|_| EndpointDriverError::Rejected)?,
            asset_revisions: BTreeMap::new(),
            one_shots: Vec::new(),
            recovery_stopped: BTreeSet::new(),
            device_format: None,
            executor,
            executor_closed: false,
            owned_audio_closed: false,
            config,
            state: AudioEndpointDriverState::Created,
            output: VecDeque::new(),
            output_bytes: 0,
            output_peak_packets: 0,
            output_peak_bytes: 0,
            output_capacity_rejections: 0,
            #[cfg(feature = "inspection")]
            faults,
        })
    }

    pub const fn state(&self) -> AudioEndpointDriverState {
        self.state
    }

    pub const fn endpoint(&self) -> &AudioEndpoint {
        &self.endpoint
    }

    pub const fn mixer(&self) -> &AudioMixerEndpoint {
        &self.mixer
    }

    #[cfg(feature = "inspection")]
    pub fn install_fault_rule(&mut self, rule: FaultRule) -> Result<(), FaultInjectionError> {
        self.faults.replace(rule)
    }

    #[cfg(feature = "inspection")]
    pub const fn fault_metrics(&self) -> FaultInjectionMetrics {
        self.faults.metrics()
    }

    pub fn queue_metrics(&self) -> EndpointQueueMetrics {
        EndpointQueueMetrics {
            packets: self.output.len(),
            peak_packets: self.output_peak_packets,
            bytes: self.output_bytes,
            peak_bytes: self.output_peak_bytes,
            capacity_rejections: self.output_capacity_rejections,
        }
    }

    pub fn owner_snapshot(&self) -> AudioEndpointDriverOwnerSnapshot {
        AudioEndpointDriverOwnerSnapshot {
            state: self.state,
            endpoint_state: self.endpoint.state(),
            device_configured: self.device_format.is_some(),
            control_revision: self.mixer.control_revision(),
            queued_events: self.endpoint.queued_events(),
            queued_event_bytes: self.endpoint.queued_event_bytes(),
            buses: self.mixer.bus_count(),
            persistent_sources: self.mixer.persistent_source_count(),
            active_voices: self.mixer.active_voice_count(),
            bound_streams: self.pipeline.bound_sources().len(),
            resident_assets: self.assets.asset_count(),
            resident_asset_bytes: self.assets.total_bytes(),
            one_shots: self.one_shots.len(),
            recovery_stopped_sources: self.recovery_stopped.len(),
            control_entries: self
                .control_snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.entries.len()),
            retiring_control_entries: self.retiring_audio_entries.len(),
            asset_revisions: self.asset_revisions.len(),
            executor_closed: self.executor_closed,
            owned_audio_closed: self.owned_audio_closed,
            output: self.queue_metrics(),
        }
    }

    pub const fn pipeline(&self) -> &AudioPipeline {
        &self.pipeline
    }

    pub const fn assets(&self) -> &AudioAssetStore {
        &self.assets
    }

    pub fn register_audio_asset(
        &mut self,
        asset: AssetRef,
        bytes: Arc<[u8]>,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_audio_open()?;
        let affected = self
            .pipeline
            .bound_sources()
            .filter_map(|(source, bound)| (bound == asset).then_some(source))
            .collect::<Vec<_>>();
        let affected_one_shots = self
            .one_shots
            .iter()
            .filter_map(|playback| (playback.asset == asset).then_some(playback.voice))
            .collect::<Vec<_>>();
        self.one_shots.retain(|playback| playback.asset != asset);
        for voice in affected_one_shots {
            self.mixer
                .stop_voice(voice)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.assets
            .insert(asset, bytes)
            .map_err(|_| EndpointDriverError::Rejected)?;
        for source in affected {
            self.pipeline
                .detach_source(source)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.reconcile_streams()
    }

    pub fn remove_audio_asset(&mut self, asset: AssetRef) -> Result<(), EndpointDriverError> {
        self.require_owned_audio_open()?;
        let affected = self
            .pipeline
            .bound_sources()
            .filter_map(|(source, bound)| (bound == asset).then_some(source))
            .collect::<Vec<_>>();
        let affected_one_shots = self
            .one_shots
            .iter()
            .filter_map(|playback| (playback.asset == asset).then_some(playback.voice))
            .collect::<Vec<_>>();
        self.one_shots.retain(|playback| playback.asset != asset);
        for voice in affected_one_shots {
            self.mixer
                .stop_voice(voice)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        for source in affected {
            self.pipeline
                .detach_source(source)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.assets
            .remove(asset)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.reconcile_streams()
    }

    pub fn mix_and_submit(
        &mut self,
        inputs: &[AudioMixInput<'_>],
        frames: usize,
    ) -> Result<AudioMixReport, EndpointDriverError> {
        if self.state != AudioEndpointDriverState::Running {
            return Err(EndpointDriverError::Rejected);
        }
        let samples = frames.checked_mul(2).ok_or(EndpointDriverError::Rejected)?;
        let mut output = vec![0_i16; samples];
        let report = self
            .mixer
            .mix_stereo_into(inputs, &mut output)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.executor
            .submit_pcm(&output)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.observed_domain_frame = self.observed_domain_frame.saturating_add(frames as u64);
        Ok(report)
    }

    pub fn render_pipeline_and_submit(
        &mut self,
        pipeline: &mut AudioPipeline,
        frames: usize,
    ) -> Result<AudioPipelineRender, EndpointDriverError> {
        if self.state != AudioEndpointDriverState::Running {
            return Err(EndpointDriverError::Rejected);
        }
        let rendered = pipeline
            .render(&mut self.mixer, frames)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.executor
            .submit_pcm(&rendered.samples)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.observed_domain_frame = self.observed_domain_frame.saturating_add(frames as u64);
        Ok(rendered)
    }

    pub fn pump_owned_pipeline(
        &mut self,
        frames: usize,
    ) -> Result<AudioPipelineRender, EndpointDriverError> {
        if self.state != AudioEndpointDriverState::Running
            || self.endpoint.state() != AudioEndpointState::Active
        {
            return Err(EndpointDriverError::Rejected);
        }
        self.decode_ready_streams()?;
        let mut one_shot_inputs = Vec::with_capacity(self.one_shots.len());
        let mut finished = Vec::new();
        for playback in &mut self.one_shots {
            let (mut samples, end_of_stream) = self
                .assets
                .read_frames(playback.asset, playback.cursor_frame, frames)
                .map_err(|_| EndpointDriverError::Failed)?;
            let rendered_frames = samples.len() / usize::from(playback.channels);
            playback.cursor_frame = playback
                .cursor_frame
                .checked_add(rendered_frames as u64)
                .ok_or(EndpointDriverError::Failed)?;
            samples.resize(frames * usize::from(playback.channels), 0);
            one_shot_inputs.push(AudioPipelineInput {
                voice: playback.voice,
                channels: playback.channels,
                samples,
            });
            if end_of_stream {
                finished.push(playback.voice);
            }
        }
        let rendered = self
            .pipeline
            .render_with_inputs(&mut self.mixer, frames, one_shot_inputs)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.one_shots
            .retain(|playback| !finished.contains(&playback.voice));
        for voice in finished {
            self.mixer
                .stop_voice(voice)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.executor
            .submit_pcm(&rendered.samples)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.observed_domain_frame = self.observed_domain_frame.saturating_add(frames as u64);
        Ok(rendered)
    }

    fn apply_audio_control_state(
        &mut self,
        state: AudioControlState,
    ) -> Result<(), EndpointDriverError> {
        self.recovery_stopped.clear();
        if state.revision == self.mixer.control_revision() {
            self.mixer
                .replace_control_state_at_current_revision(state)
                .map_err(|_| EndpointDriverError::Rejected)?;
        } else {
            self.mixer
                .apply_control_state(state)
                .map_err(|_| EndpointDriverError::Rejected)?;
        }
        self.reconcile_streams()
    }

    fn physical_audio_control_state(
        snapshot: &ControlStateSnapshot,
        retiring: &BTreeMap<StableControlRef, (RetirementFence, ControlSnapshotEntry)>,
        config: AudioControlDecodeConfig,
    ) -> Result<AudioControlState, EndpointDriverError> {
        let mut physical = snapshot.clone();
        physical
            .entries
            .retain(|entry| matches!(entry.state, ControlSnapshotState::Desired));
        physical.entries.extend(retiring.values().map(|(_, entry)| {
            let mut entry = entry.clone();
            entry.state = ControlSnapshotState::Desired;
            entry
        }));
        physical.entries.sort_by_key(|entry| entry.stable);
        decode_audio_control_snapshot(&physical, config).map_err(|_| EndpointDriverError::Rejected)
    }

    fn release_ready_audio_controls(&mut self) -> Result<(), EndpointDriverError> {
        let progress = RetirementProgress {
            render_revision: 0,
            domain_frame: self.observed_domain_frame,
            event_sequence: 0,
            audio_event_sequence: self.observed_audio_event_sequence,
        };
        let before = self.retiring_audio_entries.len();
        self.retiring_audio_entries
            .retain(|_, (fence, _)| !retirement_fence_reached(*fence, progress));
        if self.retiring_audio_entries.len() == before {
            return Ok(());
        }
        let snapshot = self
            .control_snapshot
            .as_ref()
            .ok_or(EndpointDriverError::Failed)?;
        let physical = Self::physical_audio_control_state(
            snapshot,
            &self.retiring_audio_entries,
            self.config.control_decode,
        )?;
        self.apply_audio_control_state(physical)
    }

    fn reconcile_streams(&mut self) -> Result<(), EndpointDriverError> {
        let stale = self
            .pipeline
            .bound_sources()
            .filter_map(|(source, asset)| {
                let desired = self.mixer.persistent_source(source);
                (desired.is_none_or(|desired| desired.asset != asset)
                    || !self.assets.contains(asset))
                .then_some(source)
            })
            .collect::<Vec<_>>();
        for source in stale {
            self.pipeline
                .detach_source(source)
                .map_err(|_| EndpointDriverError::Failed)?;
        }

        let desired = self.mixer.persistent_sources().copied().collect::<Vec<_>>();
        for source in desired {
            if self.pipeline.stream_for_source(source.source).is_some()
                || !self.assets.contains(source.asset)
                || self.recovery_stopped.contains(&source.source)
            {
                continue;
            }
            let format = self
                .assets
                .format(source.asset)
                .map_err(|_| EndpointDriverError::Rejected)?;
            if !matches!(format.channels, 1 | 2)
                || source.spatial.is_some() && format.channels != 1
                || self
                    .device_format
                    .is_some_and(|device| device.sample_rate != format.sample_rate)
            {
                return Err(EndpointDriverError::Rejected);
            }
            self.pipeline
                .bind_source(
                    &mut self.mixer,
                    &source,
                    format.sample_rate,
                    format.channels,
                    self.config.stream_ring_frames,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
        }
        Ok(())
    }

    fn decode_ready_streams(&mut self) -> Result<(), EndpointDriverError> {
        let sources = self.pipeline.bound_sources().collect::<Vec<_>>();
        for (source, asset) in sources {
            for _ in 0..self.config.max_decode_passes_per_wake {
                let work = match self.pipeline.request_decode(source) {
                    Ok(work) => work,
                    Err(crate::audio_pipeline::AudioPipelineError::Streaming(
                        AudioStreamingError::DecodeUnavailable
                        | AudioStreamingError::DecodeAlreadyPending,
                    )) => break,
                    Err(_) => return Err(EndpointDriverError::Failed),
                };
                match self.assets.decode(asset, work) {
                    Ok(completion) => {
                        let end = completion.end_of_stream;
                        self.pipeline
                            .complete_decode(completion)
                            .map_err(|_| EndpointDriverError::Failed)?;
                        if end {
                            break;
                        }
                    }
                    Err(_) => {
                        self.pipeline
                            .fail_decode(work)
                            .map_err(|_| EndpointDriverError::Failed)?;
                        return Err(EndpointDriverError::Rejected);
                    }
                }
            }
        }
        Ok(())
    }

    fn push(&mut self, packet: OwnedEndpointPacket) -> Result<(), EndpointDriverError> {
        packet.validate().map_err(|_| EndpointDriverError::Failed)?;
        if self.output.len() == self.config.max_output_packets {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Failed);
        }
        let Some(bytes) = self
            .output_bytes
            .checked_add(packet.payload.len())
            .filter(|bytes| *bytes <= self.config.max_output_bytes)
        else {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Failed);
        };
        self.output.push_back(packet);
        self.output_bytes = bytes;
        self.output_peak_packets = self.output_peak_packets.max(self.output.len());
        self.output_peak_bytes = self.output_peak_bytes.max(self.output_bytes);
        Ok(())
    }

    fn push_empty(
        &mut self,
        kind: MessageKind,
        source: PacketHeader,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind,
                engine: self.engine,
                channel_epoch: source.channel_epoch,
                commit_id: source.commit_id,
                base_revision: source.base_revision,
                new_revision: source.new_revision,
                required_control_revision: 0,
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: 0,
            },
            payload: Vec::new(),
        })
    }

    fn push_event_result(
        &mut self,
        source: PacketHeader,
        result: crate::audio::AudioEventResult,
    ) -> Result<(), EndpointDriverError> {
        let sequence = result.sequence;
        let payload = vec![outcome_tag(result.outcome)];
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AudioEventResult,
                engine: self.engine,
                channel_epoch: source.channel_epoch,
                commit_id: result.event_id,
                base_revision: 0,
                new_revision: self.mixer.control_revision(),
                required_control_revision: 0,
                source_simulation_revision: 0,
                sequence: result.sequence,
                payload_len: 1,
            },
            payload,
        })?;
        self.observed_audio_event_sequence = self.observed_audio_event_sequence.max(sequence);
        Ok(())
    }

    fn publish_control_observation(&mut self) -> Result<(), EndpointDriverError> {
        let Some(source) = self.last_control_source else {
            return Ok(());
        };
        let progress = RetirementProgress {
            render_revision: 0,
            domain_frame: self.observed_domain_frame,
            event_sequence: 0,
            audio_event_sequence: self.observed_audio_event_sequence,
        };
        if self.last_observed_progress == Some((source.commit_id, source.new_revision, progress)) {
            return Ok(());
        }
        self.push(
            encode_control_observed(source, ControlDomain::Audio, progress)
                .map_err(|_| EndpointDriverError::Failed)?,
        )?;
        self.last_observed_progress = Some((source.commit_id, source.new_revision, progress));
        Ok(())
    }

    fn drain_events(&mut self, source: PacketHeader) -> Result<(), EndpointDriverError> {
        loop {
            match self.endpoint.poll() {
                Some(AudioPoll::Terminal(result)) => self.push_event_result(source, result)?,
                Some(AudioPoll::Dispatch(event)) => {
                    let descriptor = decode_one_shot_event(&event);
                    let execution = if descriptor.is_some_and(|descriptor| {
                        self.validate_one_shot_control(descriptor).is_err()
                            || self.validate_one_shot_asset(descriptor).is_err()
                    }) {
                        AudioEventExecution {
                            outcome: AudioEventOutcome::FailedControlUnavailable,
                            voice: None,
                        }
                    } else {
                        self.executor.execute(&event, &mut self.mixer)
                    };
                    if let (Some(descriptor), Some(voice)) = (descriptor, execution.voice) {
                        let format = self
                            .validate_one_shot_asset(descriptor)
                            .map_err(|_| EndpointDriverError::Failed)?;
                        self.one_shots.push(OneShotPlayback {
                            voice,
                            asset: descriptor.asset,
                            channels: format.channels,
                            cursor_frame: 0,
                        });
                    }
                    let result = self
                        .endpoint
                        .complete(event.sequence, execution.outcome)
                        .map_err(|_| EndpointDriverError::Failed)?;
                    self.push_event_result(source, result)?;
                }
                None => return Ok(()),
            }
        }
    }

    fn fail<T>(&mut self) -> Result<T, EndpointDriverError> {
        self.state = AudioEndpointDriverState::Failed;
        Err(EndpointDriverError::Failed)
    }

    fn require_owned_audio_open(&self) -> Result<(), EndpointDriverError> {
        if self.owned_audio_closed
            || matches!(
                self.state,
                AudioEndpointDriverState::Closed | AudioEndpointDriverState::Failed
            )
        {
            Err(EndpointDriverError::Rejected)
        } else {
            Ok(())
        }
    }

    fn shutdown_owned_audio(
        &mut self,
        deadline_micros: u64,
    ) -> Result<Option<AudioEndpointDriverShutdownReport>, EndpointDriverError> {
        if self.owned_audio_closed {
            self.close_executor(deadline_micros)?;
            return Ok(None);
        }

        self.endpoint
            .preflight_shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        self.pipeline
            .streaming()
            .preflight_shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        let voices = self
            .pipeline
            .bound_voice_handles()
            .chain(self.one_shots.iter().map(|playback| playback.voice))
            .collect::<Vec<_>>();
        self.mixer
            .preflight_stop_voices(&voices)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.mixer
            .preflight_shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;

        self.close_executor(deadline_micros)?;
        let endpoint = self
            .endpoint
            .shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        let pipeline = self
            .pipeline
            .shutdown(&mut self.mixer)
            .map_err(|_| EndpointDriverError::Failed)?;
        let released_one_shots = self.one_shots.len();
        for playback in self.one_shots.drain(..) {
            self.mixer
                .stop_voice(playback.voice)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        let mixer = self
            .mixer
            .shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        let assets = self.assets.shutdown();

        let released_control_entries = self
            .control_snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.entries.len());
        let released_retiring_control_entries = self.retiring_audio_entries.len();
        let released_asset_revisions = self.asset_revisions.len();
        let released_recovery_stops = self.recovery_stopped.len();
        self.control = None;
        self.control_snapshot = None;
        self.retiring_audio_entries.clear();
        self.last_control_source = None;
        self.last_observed_progress = None;
        self.observed_domain_frame = 0;
        self.observed_audio_event_sequence = 0;
        self.asset_revisions.clear();
        self.recovery_stopped.clear();
        self.device_format = None;
        #[cfg(feature = "inspection")]
        self.faults.shutdown();
        self.owned_audio_closed = true;

        Ok(Some(AudioEndpointDriverShutdownReport {
            endpoint,
            pipeline,
            mixer,
            assets,
            released_one_shots,
            released_control_entries,
            released_retiring_control_entries,
            released_asset_revisions,
            released_recovery_stops,
        }))
    }

    fn close_executor(&mut self, deadline_micros: u64) -> Result<(), EndpointDriverError> {
        if self.executor_closed {
            return Ok(());
        }
        self.executor
            .close(deadline_micros)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.executor_closed = true;
        Ok(())
    }

    fn validate_one_shot_asset(
        &self,
        descriptor: AudioOneShotDescriptor,
    ) -> Result<crate::audio_device_adapter::Pcm16AudioFormat, ()> {
        let format = self.assets.format(descriptor.asset).map_err(|_| ())?;
        if !matches!(format.channels, 1 | 2)
            || descriptor.spatial.is_some() && format.channels != 1
            || self
                .device_format
                .is_some_and(|device| device.sample_rate != format.sample_rate)
        {
            return Err(());
        }
        Ok(format)
    }

    fn validate_one_shot_control(&self, descriptor: AudioOneShotDescriptor) -> Result<(), ()> {
        self.control
            .as_ref()
            .filter(|control| control.buses.iter().any(|bus| bus.bus == descriptor.bus))
            .map(|_| ())
            .ok_or(())
    }
}

impl EndpointDriver for AudioEndpointDriver {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError> {
        if header.engine != self.engine {
            return self.fail();
        }
        #[cfg(feature = "inspection")]
        if let Some(fault) = self.faults.trigger(audio_fault_point(header.kind)) {
            match fault {
                InjectedFault::RejectBeforeDispatch | InjectedFault::DropLatestOnly => {
                    return Err(EndpointDriverError::Rejected);
                }
                InjectedFault::FailOwner
                | InjectedFault::OutcomeUnknown
                | InjectedFault::SurfaceLost
                | InjectedFault::DeviceLost
                | InjectedFault::AudioDeviceLost => return self.fail(),
            }
        }
        match header.kind {
            MessageKind::EngineStart if self.state == AudioEndpointDriverState::Created => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = AudioEndpointDriverState::Running;
                self.push_empty(MessageKind::EngineReady, header)?;
            }
            MessageKind::AudioControlTransaction
                if self.state == AudioEndpointDriverState::Running =>
            {
                let snapshot = decode_audio_control_snapshot_parts(
                    header,
                    payload,
                    self.config.control_decode.max_entries,
                    self.config.control_decode.max_descriptor_bytes,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                let state = decode_audio_control_snapshot(&snapshot, self.config.control_decode)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if snapshot.revision < self.mixer.control_revision() {
                    return Err(EndpointDriverError::Rejected);
                }
                if snapshot.revision == self.mixer.control_revision() {
                    let current_snapshot = self
                        .control_snapshot
                        .as_ref()
                        .ok_or(EndpointDriverError::Rejected)?;
                    if current_snapshot != &snapshot
                        && !is_tombstone_update(current_snapshot, &snapshot)
                    {
                        return Err(EndpointDriverError::Rejected);
                    }
                }
                let realized = state
                    .buses
                    .iter()
                    .map(|bus| bus.bus)
                    .chain(state.sources.iter().map(|source| source.source))
                    .collect::<Vec<_>>();
                let retiring_audio_entries = snapshot
                    .entries
                    .iter()
                    .filter_map(|entry| match &entry.state {
                        ControlSnapshotState::Tombstone { fence } => {
                            Some((entry.stable, (*fence, entry.clone())))
                        }
                        ControlSnapshotState::Desired => None,
                    })
                    .collect::<BTreeMap<_, _>>();
                let physical = Self::physical_audio_control_state(
                    &snapshot,
                    &retiring_audio_entries,
                    self.config.control_decode,
                )?;
                self.apply_audio_control_state(physical)?;
                self.control = Some(state);
                self.control_snapshot = Some(snapshot.clone());
                self.retiring_audio_entries = retiring_audio_entries;
                self.endpoint.set_audio_control_revision(snapshot.revision);
                self.push_empty(MessageKind::AudioControlAck, header)?;
                self.push(
                    encode_control_realization_results(
                        header,
                        ControlDomain::Audio,
                        self.endpoint.endpoint_generation(),
                        &realized
                            .iter()
                            .copied()
                            .map(|stable| (stable, RealizationOutcome::Live))
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|_| EndpointDriverError::Failed)?,
                )?;
                self.drain_events(header)?;
                self.last_control_source = Some(header);
            }
            #[cfg(feature = "inspection")]
            MessageKind::InspectionRequest
                if self.state == AudioEndpointDriverState::Running
                    && is_fault_wire_command(payload) =>
            {
                let response = apply_fault_wire_command(&mut self.faults, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push(fault_wire_response_packet(self.engine, header, response))?;
            }
            MessageKind::AudioAssetData if self.state == AudioEndpointDriverState::Running => {
                let command = decode_audio_asset_command(self.engine, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let (asset, revision) = command.identity();
                if header.commit_id != handle_key(asset.handle)
                    || header.new_revision != revision
                    || self
                        .asset_revisions
                        .get(&asset)
                        .is_some_and(|current| *current >= revision)
                {
                    return Err(EndpointDriverError::Rejected);
                }
                match command {
                    AudioAssetCommand::Upsert { bytes, .. } => {
                        self.register_audio_asset(asset, bytes)?;
                    }
                    AudioAssetCommand::Remove { .. } => {
                        self.remove_audio_asset(asset)?;
                    }
                }
                self.asset_revisions.insert(asset, revision);
                self.push_empty(MessageKind::AudioAssetAck, header)?;
            }
            MessageKind::AudioEvent if self.state == AudioEndpointDriverState::Running => {
                if header.sequence == 0
                    || header.commit_id == 0
                    || header.new_revision == 0
                    || header.source_simulation_revision == 0
                {
                    return Err(EndpointDriverError::Rejected);
                }
                let terminal = self
                    .endpoint
                    .stage_event(AudioEvent {
                        engine: self.engine,
                        sequence: header.sequence,
                        event_id: header.commit_id,
                        tick_id: header.source_simulation_revision,
                        required_audio_control_revision: header.required_control_revision,
                        deadline_millis: header.new_revision,
                        payload: payload.to_vec(),
                    })
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if let Some(result) = terminal {
                    self.push_event_result(header, result)?;
                }
                self.drain_events(header)?;
            }
            MessageKind::ControlObservedAck if self.state == AudioEndpointDriverState::Running => {
                let observed = decode_control_observed_ack(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if observed.domain != ControlDomain::Audio
                    || self.last_control_source.is_none_or(|source| {
                        observed.revision > source.new_revision
                            || observed.revision == source.new_revision
                                && observed.transaction_id != source.commit_id
                    })
                {
                    return Err(EndpointDriverError::Rejected);
                }
            }
            MessageKind::DeviceEvent if self.state == AudioEndpointDriverState::Running => {
                self.apply_device_event(payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.drain_events(header)?;
            }
            MessageKind::WorkerWake if self.state == AudioEndpointDriverState::Running => {
                if !payload.is_empty() {
                    return Err(EndpointDriverError::Rejected);
                }
                if self.endpoint.state() == AudioEndpointState::Active {
                    self.pump_owned_pipeline(self.config.worker_render_frames)?;
                } else {
                    self.decode_ready_streams()?;
                }
                self.drain_events(header)?;
            }
            MessageKind::EngineSuspend if self.state == AudioEndpointDriverState::Running => {
                if !payload.is_empty() {
                    return self.fail();
                }
                if self.endpoint.state() == AudioEndpointState::Active {
                    self.endpoint
                        .suspend()
                        .map_err(|_| EndpointDriverError::Failed)?;
                    self.executor
                        .suspend()
                        .map_err(|_| EndpointDriverError::Failed)?;
                }
            }
            MessageKind::EngineResume if self.state == AudioEndpointDriverState::Running => {
                if !matches!(payload.len(), 0 | 58) {
                    return self.fail();
                }
                if self.endpoint.state() == AudioEndpointState::Suspended {
                    let permit = if payload.len() == 58 {
                        let permit = decode_audio_permit(payload, 0)
                            .map_err(|_| EndpointDriverError::Rejected)?;
                        self.validate_device_format(permit.format)
                            .map_err(|_| EndpointDriverError::Rejected)?;
                        Some(permit)
                    } else {
                        None
                    };
                    self.endpoint
                        .resume()
                        .map_err(|_| EndpointDriverError::Failed)?;
                    if let Some(permit) = permit {
                        self.device_format = Some(permit.format);
                        self.executor
                            .resume(permit)
                            .map_err(|_| EndpointDriverError::Failed)?;
                    }
                }
                self.drain_events(header)?;
            }
            MessageKind::EngineClose
                if matches!(
                    self.state,
                    AudioEndpointDriverState::Created
                        | AudioEndpointDriverState::Running
                        | AudioEndpointDriverState::Failed
                ) =>
            {
                if !payload.is_empty() {
                    return self.fail();
                }
                let shutdown = self
                    .shutdown_owned_audio(0)?
                    .ok_or(EndpointDriverError::Failed)?;
                for result in shutdown.endpoint.terminal_events {
                    self.push_event_result(header, result)?;
                }
                self.state = AudioEndpointDriverState::Closed;
                self.push_empty(MessageKind::EngineClosed, header)?;
            }
            _ => return Err(EndpointDriverError::Rejected),
        }
        self.release_ready_audio_controls()?;
        self.publish_control_observation()?;
        Ok(if self.output.is_empty() {
            DispatchOutcome::Idle
        } else {
            DispatchOutcome::Wake
        })
    }

    fn poll(&mut self, budget: usize) -> Result<Vec<OwnedEndpointPacket>, EndpointDriverError> {
        let packets = (0..budget)
            .map_while(|_| self.output.pop_front())
            .collect::<Vec<_>>();
        self.output_bytes = self.output.iter().map(|packet| packet.payload.len()).sum();
        Ok(packets)
    }

    fn close(&mut self, deadline_micros: u64) -> Result<(), EndpointDriverError> {
        self.shutdown_owned_audio(deadline_micros)?;
        self.state = AudioEndpointDriverState::Closed;
        self.output.clear();
        self.output_bytes = 0;
        Ok(())
    }
}

const fn retirement_fence_reached(fence: RetirementFence, progress: RetirementProgress) -> bool {
    progress.render_revision >= fence.last_render_revision
        && progress.domain_frame >= fence.last_domain_frame
        && progress.event_sequence >= fence.last_event_sequence
        && progress.audio_event_sequence >= fence.last_audio_event_sequence
}

#[cfg(feature = "inspection")]
const fn audio_fault_point(kind: MessageKind) -> FaultPoint {
    match kind {
        MessageKind::WorkerWake => FaultPoint::WorkerDispatch,
        MessageKind::AudioAssetData => FaultPoint::ResourceAcquire,
        MessageKind::AudioEvent | MessageKind::AudioControlTransaction => {
            FaultPoint::AudioOperation
        }
        MessageKind::DeviceEvent => FaultPoint::DeviceOperation,
        MessageKind::EngineClose => FaultPoint::Shutdown,
        _ => FaultPoint::ProtocolDecode,
    }
}

impl AudioEndpointDriver {
    fn apply_device_event(&mut self, payload: &[u8]) -> Result<(), ()> {
        let (&tag, rest) = payload.split_first().ok_or(())?;
        match tag {
            1 if matches!(rest.len(), 16 | 74) => {
                let permit = if rest.len() == 74 {
                    Some(decode_audio_permit(rest, 16)?)
                } else {
                    None
                };
                if let Some(permit) = permit {
                    self.validate_device_format(permit.format)?;
                    self.device_format = Some(permit.format);
                    self.reconcile_streams().map_err(|_| ())?;
                }
                self.endpoint
                    .activate(AudioGestureToken {
                        engine: self.engine,
                        endpoint_generation: Handle {
                            index: read_u32(rest, 0),
                            generation: read_u32(rest, 4),
                        },
                        token: Handle {
                            index: read_u32(rest, 8),
                            generation: read_u32(rest, 12),
                        },
                    })
                    .map_err(|_| ())?;
                if let Some(permit) = permit {
                    self.executor.activate(permit)
                } else {
                    Ok(())
                }
            }
            2 if rest.is_empty() => {
                self.executor.device_lost()?;
                let results = self.endpoint.device_lost().map_err(|_| ())?;
                let recovery = self.pipeline.restart(&mut self.mixer).map_err(|_| ())?;
                self.recovery_stopped.extend(recovery.stopped);
                self.one_shots.clear();
                self.device_format = None;
                for result in results {
                    self.push_event_result(
                        PacketHeader {
                            kind: MessageKind::DeviceEvent,
                            engine: self.engine,
                            channel_epoch: 0,
                            commit_id: 0,
                            base_revision: 0,
                            new_revision: 0,
                            required_control_revision: 0,
                            source_simulation_revision: 0,
                            sequence: result.sequence,
                            payload_len: 0,
                        },
                        result,
                    )
                    .map_err(|_| ())?;
                }
                Ok(())
            }
            3 if matches!(rest.len(), 0 | 58) => {
                let permit = if rest.len() == 58 {
                    let permit = decode_audio_permit(rest, 0)?;
                    self.validate_device_format(permit.format)?;
                    Some(permit)
                } else {
                    None
                };
                let results = self.endpoint.restart().map_err(|_| ())?;
                let recovery = self.pipeline.restart(&mut self.mixer).map_err(|_| ())?;
                self.recovery_stopped.extend(recovery.stopped);
                self.one_shots.clear();
                self.device_format = permit.map(|permit| permit.format);
                self.reconcile_streams().map_err(|_| ())?;
                if let Some(permit) = permit {
                    self.executor.resume(permit)?;
                }
                for result in results {
                    self.push_event_result(
                        PacketHeader {
                            kind: MessageKind::DeviceEvent,
                            engine: self.engine,
                            channel_epoch: 0,
                            commit_id: 0,
                            base_revision: 0,
                            new_revision: 0,
                            required_control_revision: 0,
                            source_simulation_revision: 0,
                            sequence: result.sequence,
                            payload_len: 0,
                        },
                        result,
                    )
                    .map_err(|_| ())?;
                }
                Ok(())
            }
            4 if rest.len() == 8 => self
                .endpoint
                .advance_time(read_u64(rest, 0))
                .map_err(|_| ()),
            _ => Err(()),
        }
    }

    fn validate_device_format(&self, format: AudioDeviceFormat) -> Result<(), ()> {
        if format.channels != 2
            || format.sample_rate < 8_000
            || format.sample_rate > 384_000
            || format.callback_frames == 0
            || self.mixer.persistent_sources().any(|source| {
                self.assets
                    .format(source.asset)
                    .is_ok_and(|asset| asset.sample_rate != format.sample_rate)
            })
        {
            return Err(());
        }
        Ok(())
    }
}

pub fn encode_audio_asset_completion(asset: AssetRef, bytes: &[u8]) -> Option<Vec<u8>> {
    encode_audio_asset_revision(asset, 1, bytes)
}

pub fn encode_audio_asset_revision(
    asset: AssetRef,
    revision: u64,
    bytes: &[u8],
) -> Option<Vec<u8>> {
    let byte_len = u32::try_from(bytes.len()).ok()?;
    if !asset.engine.is_valid() || !asset.handle.is_valid() || revision == 0 || bytes.is_empty() {
        return None;
    }
    let mut payload = Vec::with_capacity(25 + bytes.len());
    payload.extend_from_slice(b"VPA2");
    payload.push(1);
    payload.extend_from_slice(&asset.handle.index.to_le_bytes());
    payload.extend_from_slice(&asset.handle.generation.to_le_bytes());
    payload.extend_from_slice(&revision.to_le_bytes());
    payload.extend_from_slice(&byte_len.to_le_bytes());
    payload.extend_from_slice(bytes);
    Some(payload)
}

enum AudioAssetCommand {
    Upsert {
        asset: AssetRef,
        revision: u64,
        bytes: Arc<[u8]>,
    },
    Remove {
        asset: AssetRef,
        revision: u64,
    },
}

impl AudioAssetCommand {
    const fn identity(&self) -> (AssetRef, u64) {
        match self {
            Self::Upsert {
                asset, revision, ..
            }
            | Self::Remove { asset, revision } => (*asset, *revision),
        }
    }
}

fn decode_audio_asset_command(engine: EngineId, payload: &[u8]) -> Result<AudioAssetCommand, ()> {
    if payload.len() < 25 || payload.get(..4) != Some(b"VPA2") {
        return Err(());
    }
    let action = payload[4];
    let asset = AssetRef {
        engine,
        handle: Handle {
            index: read_u32(payload, 5),
            generation: read_u32(payload, 9),
        },
    };
    let revision = read_u64(payload, 13);
    let byte_len = read_u32(payload, 21) as usize;
    if !asset.handle.is_valid() || revision == 0 || payload.len().checked_sub(25) != Some(byte_len)
    {
        return Err(());
    }
    match action {
        1 if byte_len != 0 => Ok(AudioAssetCommand::Upsert {
            asset,
            revision,
            bytes: Arc::from(&payload[25..]),
        }),
        2 if byte_len == 0 => Ok(AudioAssetCommand::Remove { asset, revision }),
        _ => Err(()),
    }
}

fn handle_key(handle: Handle) -> u64 {
    u64::from(handle.index) | (u64::from(handle.generation) << 32)
}

fn outcome_tag(outcome: AudioEventOutcome) -> u8 {
    match outcome {
        AudioEventOutcome::Executed => 1,
        AudioEventOutcome::DroppedBeforeDispatch => 2,
        AudioEventOutcome::OutcomeUnknown => 3,
        AudioEventOutcome::FailedControlUnavailable => 4,
        AudioEventOutcome::AudioLocked => 5,
        AudioEventOutcome::DeadlineExceeded => 6,
        AudioEventOutcome::Failed => 7,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn decode_audio_permit(bytes: &[u8], offset: usize) -> Result<AudioDevicePermit, ()> {
    let body = bytes.get(offset..offset + 58).ok_or(())?;
    let permit = AudioDevicePermit {
        lease: GenerationalHandle {
            index: read_u32(body, 0),
            generation: read_u32(body, 4),
        },
        realtime: AudioRealtimeEndpoint {
            session: GenerationalHandle {
                index: read_u32(body, 8),
                generation: read_u32(body, 12),
            },
            session_epoch: read_u64(body, 16),
            endpoint: GenerationalHandle {
                index: read_u32(body, 24),
                generation: read_u32(body, 28),
            },
            endpoint_epoch: read_u64(body, 32),
        },
        device_generation: GenerationalHandle {
            index: read_u32(body, 40),
            generation: read_u32(body, 44),
        },
        format: AudioDeviceFormat {
            sample_rate: read_u32(body, 48),
            channels: u16::from_le_bytes(body[52..54].try_into().map_err(|_| ())?),
            callback_frames: read_u32(body, 54),
        },
    };
    if !permit.lease.is_valid()
        || !permit.realtime.is_valid()
        || !permit.device_generation.is_valid()
        || permit.format.sample_rate == 0
        || permit.format.channels == 0
        || permit.format.callback_frames == 0
    {
        return Err(());
    }
    Ok(permit)
}

pub fn encode_audio_device_activation(
    gesture: AudioGestureToken,
    permit: AudioDevicePermit,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(75);
    payload.push(1);
    push_handle(&mut payload, gesture.endpoint_generation);
    push_handle(&mut payload, gesture.token);
    encode_audio_permit(&mut payload, permit);
    payload
}

pub fn encode_audio_device_lost() -> Vec<u8> {
    vec![2]
}

pub fn encode_audio_device_recovery(permit: AudioDevicePermit) -> Vec<u8> {
    let mut payload = Vec::with_capacity(59);
    payload.push(3);
    encode_audio_permit(&mut payload, permit);
    payload
}

pub fn encode_audio_resume(permit: AudioDevicePermit) -> Vec<u8> {
    let mut payload = Vec::with_capacity(58);
    encode_audio_permit(&mut payload, permit);
    payload
}

fn encode_audio_permit(output: &mut Vec<u8>, permit: AudioDevicePermit) {
    push_generational_handle(output, permit.lease);
    push_generational_handle(output, permit.realtime.session);
    output.extend_from_slice(&permit.realtime.session_epoch.to_le_bytes());
    push_generational_handle(output, permit.realtime.endpoint);
    output.extend_from_slice(&permit.realtime.endpoint_epoch.to_le_bytes());
    push_generational_handle(output, permit.device_generation);
    output.extend_from_slice(&permit.format.sample_rate.to_le_bytes());
    output.extend_from_slice(&permit.format.channels.to_le_bytes());
    output.extend_from_slice(&permit.format.callback_frames.to_le_bytes());
}

fn push_handle(output: &mut Vec<u8>, handle: Handle) {
    output.extend_from_slice(&handle.index.to_le_bytes());
    output.extend_from_slice(&handle.generation.to_le_bytes());
}

fn push_generational_handle(output: &mut Vec<u8>, handle: GenerationalHandle) {
    output.extend_from_slice(&handle.index.to_le_bytes());
    output.extend_from_slice(&handle.generation.to_le_bytes());
}

fn decode_one_shot_event(event: &AudioEvent) -> Option<AudioOneShotDescriptor> {
    let bytes = event.payload.as_slice();
    if bytes.len() < 24 || &bytes[..4] != b"VAE2" || bytes[4] != 1 {
        return None;
    }
    let asset = crate::asset::AssetRef {
        engine: event.engine,
        handle: Handle {
            index: read_u32(bytes, 5),
            generation: read_u32(bytes, 9),
        },
    };
    let bus = StableControlRef {
        engine: event.engine,
        kind: ControlKind::AudioBus,
        handle: Handle {
            index: read_u32(bytes, 13),
            generation: read_u32(bytes, 17),
        },
    };
    let gain_q15 = u16::from_le_bytes(bytes[21..23].try_into().ok()?);
    let spatial = match bytes[23] {
        0 if bytes.len() == 24 => None,
        1 if bytes.len() == 44 => Some(AudioSpatialSource {
            position_mm: [
                i32::from_le_bytes(bytes[24..28].try_into().ok()?),
                i32::from_le_bytes(bytes[28..32].try_into().ok()?),
                i32::from_le_bytes(bytes[32..36].try_into().ok()?),
            ],
            min_distance_mm: read_u32(bytes, 36),
            max_distance_mm: read_u32(bytes, 40),
        }),
        _ => return None,
    };
    Some(AudioOneShotDescriptor {
        event_id: event.event_id,
        asset,
        bus,
        gain_q15,
        spatial,
    })
}
