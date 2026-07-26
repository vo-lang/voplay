use std::collections::BTreeMap;

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::AssetRef,
    audio_mixer::{
        AudioMixInput, AudioMixReport, AudioMixerEndpoint, AudioMixerError, AudioVoiceHandle,
        PersistentAudioSourceDescriptor,
    },
    audio_streaming::{
        AudioDecodeCompletion, AudioDecodeWork, AudioRenderReport, AudioStreamHandle,
        AudioStreamRecoveryPlan, AudioStreamingConfig, AudioStreamingEndpoint, AudioStreamingError,
        AudioStreamingOwnerSnapshot, AudioStreamingShutdownReport,
    },
    control::StableControlRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPipelineConfig {
    pub streaming: AudioStreamingConfig,
    pub max_sources: usize,
    pub max_render_frames: usize,
}

impl Default for AudioPipelineConfig {
    fn default() -> Self {
        Self {
            streaming: AudioStreamingConfig::default(),
            max_sources: 64,
            max_render_frames: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioPipelineError {
    InvalidConfig,
    DuplicateSource,
    SourceCapacity,
    UnknownSource,
    RenderCapacity,
    ArithmeticOverflow,
    Streaming(AudioStreamingError),
    Mixer(AudioMixerError),
}

impl From<AudioStreamingError> for AudioPipelineError {
    fn from(error: AudioStreamingError) -> Self {
        Self::Streaming(error)
    }
}

impl From<AudioMixerError> for AudioPipelineError {
    fn from(error: AudioMixerError) -> Self {
        Self::Mixer(error)
    }
}

#[derive(Clone, Copy, Debug)]
struct SourceBinding {
    stream: AudioStreamHandle,
    voice: AudioVoiceHandle,
    asset: AssetRef,
    channels: u16,
}

#[derive(Clone, Debug)]
pub struct AudioPipelineRender {
    pub mix: AudioMixReport,
    pub streams: Vec<AudioRenderReport>,
    pub samples: Vec<i16>,
}

#[derive(Clone, Debug)]
pub struct AudioPipelineInput {
    pub voice: AudioVoiceHandle,
    pub channels: u16,
    pub samples: Vec<i16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioPipelineRecovery {
    pub endpoint_generation: Handle,
    pub restored: Vec<StableControlRef>,
    pub stopped: Vec<StableControlRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPipelineOwnerSnapshot {
    pub bound_sources: usize,
    pub streaming: AudioStreamingOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioPipelineShutdownReport {
    pub released_sources: Vec<StableControlRef>,
    pub streaming: AudioStreamingShutdownReport,
}

pub struct AudioPipeline {
    config: AudioPipelineConfig,
    streaming: AudioStreamingEndpoint,
    sources: BTreeMap<StableControlRef, SourceBinding>,
}

impl AudioPipeline {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        decode_actor: Handle,
        config: AudioPipelineConfig,
    ) -> Result<Self, AudioPipelineError> {
        if config.max_sources == 0
            || config.max_sources > config.streaming.max_streams
            || config.max_render_frames == 0
            || config.max_render_frames > config.streaming.max_callback_frames
        {
            return Err(AudioPipelineError::InvalidConfig);
        }
        Ok(Self {
            streaming: AudioStreamingEndpoint::new(
                engine,
                endpoint_generation,
                decode_actor,
                config.streaming,
            )?,
            config,
            sources: BTreeMap::new(),
        })
    }

    pub const fn streaming(&self) -> &AudioStreamingEndpoint {
        &self.streaming
    }

    pub fn owner_snapshot(&self) -> AudioPipelineOwnerSnapshot {
        AudioPipelineOwnerSnapshot {
            bound_sources: self.sources.len(),
            streaming: self.streaming.owner_snapshot(),
        }
    }

    pub fn bound_voice_handles(&self) -> impl ExactSizeIterator<Item = AudioVoiceHandle> + '_ {
        self.sources.values().map(|binding| binding.voice)
    }

    pub fn bind_source(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
        control: &PersistentAudioSourceDescriptor,
        sample_rate: u32,
        channels: u16,
        ring_capacity_frames: usize,
    ) -> Result<AudioStreamHandle, AudioPipelineError> {
        if self.sources.contains_key(&control.source) {
            return Err(AudioPipelineError::DuplicateSource);
        }
        if self.sources.len() == self.config.max_sources {
            return Err(AudioPipelineError::SourceCapacity);
        }
        let stream = self.streaming.create_stream_from_control(
            control,
            sample_rate,
            channels,
            ring_capacity_frames,
        )?;
        let voice = match mixer.start_persistent(control.source) {
            Ok(voice) => voice,
            Err(error) => {
                let _ = self.streaming.destroy_stream(stream);
                return Err(error.into());
            }
        };
        self.sources.insert(
            control.source,
            SourceBinding {
                stream,
                voice,
                asset: control.asset,
                channels,
            },
        );
        Ok(stream)
    }

    pub fn bound_sources(
        &self,
    ) -> impl ExactSizeIterator<Item = (StableControlRef, AssetRef)> + '_ {
        self.sources
            .iter()
            .map(|(source, binding)| (*source, binding.asset))
    }

    pub fn stream_for_source(&self, source: StableControlRef) -> Option<AudioStreamHandle> {
        self.sources.get(&source).map(|binding| binding.stream)
    }

    pub fn detach_source(&mut self, source: StableControlRef) -> Result<(), AudioPipelineError> {
        let binding = self
            .sources
            .remove(&source)
            .ok_or(AudioPipelineError::UnknownSource)?;
        self.streaming.destroy_stream(binding.stream)?;
        Ok(())
    }

    pub fn unbind_source(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
        source: StableControlRef,
    ) -> Result<(), AudioPipelineError> {
        let binding = self
            .sources
            .remove(&source)
            .ok_or(AudioPipelineError::UnknownSource)?;
        let stream_result = self.streaming.destroy_stream(binding.stream);
        let voice_result = mixer.stop_voice(binding.voice);
        stream_result?;
        voice_result?;
        Ok(())
    }

    pub fn request_decode(
        &mut self,
        source: StableControlRef,
    ) -> Result<AudioDecodeWork, AudioPipelineError> {
        let stream = self
            .sources
            .get(&source)
            .ok_or(AudioPipelineError::UnknownSource)?
            .stream;
        Ok(self.streaming.request_decode(stream)?)
    }

    pub fn complete_decode(
        &mut self,
        completion: AudioDecodeCompletion,
    ) -> Result<usize, AudioPipelineError> {
        Ok(self.streaming.complete_decode(completion)?)
    }

    pub fn fail_decode(&mut self, work: AudioDecodeWork) -> Result<(), AudioPipelineError> {
        Ok(self.streaming.fail_decode(work)?)
    }

    pub fn render(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
        frames: usize,
    ) -> Result<AudioPipelineRender, AudioPipelineError> {
        self.render_with_inputs(mixer, frames, Vec::new())
    }

    pub fn render_with_inputs(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
        frames: usize,
        extra: Vec<AudioPipelineInput>,
    ) -> Result<AudioPipelineRender, AudioPipelineError> {
        if frames == 0 || frames > self.config.max_render_frames {
            return Err(AudioPipelineError::RenderCapacity);
        }
        let ordered = self.sources.values().copied().collect::<Vec<_>>();
        let mut buffers = Vec::with_capacity(ordered.len());
        let mut reports = Vec::with_capacity(ordered.len());
        for binding in &ordered {
            let samples = frames
                .checked_mul(binding.channels as usize)
                .ok_or(AudioPipelineError::ArithmeticOverflow)?;
            let mut buffer = vec![0_i16; samples];
            reports.push(self.streaming.render_into(binding.stream, &mut buffer)?);
            buffers.push(buffer);
        }
        let inputs = ordered
            .iter()
            .zip(&buffers)
            .map(|(binding, samples)| AudioMixInput {
                voice: binding.voice,
                channels: binding.channels,
                samples,
            })
            .chain(extra.iter().map(|input| AudioMixInput {
                voice: input.voice,
                channels: input.channels,
                samples: &input.samples,
            }))
            .collect::<Vec<_>>();
        let output_samples = frames
            .checked_mul(2)
            .ok_or(AudioPipelineError::ArithmeticOverflow)?;
        let mut output = vec![0_i16; output_samples];
        let mix = mixer.mix_stereo_into(&inputs, &mut output)?;
        Ok(AudioPipelineRender {
            mix,
            streams: reports,
            samples: output,
        })
    }

    pub fn restart(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
    ) -> Result<AudioPipelineRecovery, AudioPipelineError> {
        let plans = self.streaming.restart()?;
        if let Err(error) = mixer.restart() {
            self.sources.clear();
            return Err(error.into());
        }
        self.sources.clear();
        let mut restored = Vec::new();
        let mut stopped = Vec::new();
        for plan in plans {
            let source = plan.source;
            let channels = plan.descriptor.channels;
            match self.restore_binding(mixer, plan) {
                Ok(Some((stream, voice))) => {
                    self.sources.insert(
                        source,
                        SourceBinding {
                            stream,
                            voice,
                            asset: mixer
                                .persistent_source(source)
                                .ok_or(AudioPipelineError::UnknownSource)?
                                .asset,
                            channels,
                        },
                    );
                    restored.push(source);
                }
                Ok(None) => stopped.push(source),
                Err(error) => {
                    self.clear_recovery_bindings(mixer);
                    return Err(error);
                }
            }
        }
        Ok(AudioPipelineRecovery {
            endpoint_generation: self.streaming.endpoint_generation(),
            restored,
            stopped,
        })
    }

    pub fn shutdown(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
    ) -> Result<AudioPipelineShutdownReport, AudioPipelineError> {
        if self.sources.is_empty() && self.streaming.owner_snapshot().closed {
            return Ok(AudioPipelineShutdownReport {
                released_sources: Vec::new(),
                streaming: self.streaming.shutdown()?,
            });
        }
        let bindings = self
            .sources
            .iter()
            .map(|(source, binding)| (*source, *binding))
            .collect::<Vec<_>>();
        self.streaming.preflight_shutdown()?;
        mixer.preflight_stop_voices(
            &bindings
                .iter()
                .map(|(_, binding)| binding.voice)
                .collect::<Vec<_>>(),
        )?;
        for (_, binding) in &bindings {
            mixer.stop_voice(binding.voice)?;
        }
        let streaming = self.streaming.shutdown()?;
        self.sources.clear();
        Ok(AudioPipelineShutdownReport {
            released_sources: bindings.into_iter().map(|(source, _)| source).collect(),
            streaming,
        })
    }

    fn restore_binding(
        &mut self,
        mixer: &mut AudioMixerEndpoint,
        plan: AudioStreamRecoveryPlan,
    ) -> Result<Option<(AudioStreamHandle, AudioVoiceHandle)>, AudioPipelineError> {
        let source = plan.source;
        let Some(stream) = self.streaming.restore_stream(plan)? else {
            return Ok(None);
        };
        match mixer.start_persistent(source) {
            Ok(voice) => Ok(Some((stream, voice))),
            Err(error) => {
                let _ = self.streaming.destroy_stream(stream);
                Err(error.into())
            }
        }
    }

    fn clear_recovery_bindings(&mut self, mixer: &mut AudioMixerEndpoint) {
        for binding in self.sources.values().copied() {
            let _ = self.streaming.destroy_stream(binding.stream);
            let _ = mixer.stop_voice(binding.voice);
        }
        self.sources.clear();
    }
}
