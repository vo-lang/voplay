use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Take},
    path::Path,
    sync::Arc,
};

use vo_app_protocol::AudioDevicePermit;
use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::AssetRef,
    audio_mixer::{AudioMixInput, AudioMixReport, AudioMixerEndpoint, AudioMixerError},
    audio_streaming::{AudioDecodeCompletion, AudioDecodeWork},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDeviceCallbackReport {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub callback_sequence: u64,
    pub mix: AudioMixReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioDeviceAdapterError {
    InvalidEngine,
    InvalidEndpoint,
    InvalidPermit,
    StalePermit,
    UnsupportedFormat,
    CallbackShape,
    CallbackSequenceExhausted,
    Mixer(AudioMixerError),
}

impl From<AudioMixerError> for AudioDeviceAdapterError {
    fn from(error: AudioMixerError) -> Self {
        Self::Mixer(error)
    }
}

pub struct AppAudioDeviceAdapter {
    engine: EngineId,
    endpoint_generation: Handle,
    permit: AudioDevicePermit,
    callback_sequence: u64,
}

impl AppAudioDeviceAdapter {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        permit: AudioDevicePermit,
    ) -> Result<Self, AudioDeviceAdapterError> {
        if !engine.is_valid() {
            return Err(AudioDeviceAdapterError::InvalidEngine);
        }
        if !endpoint_generation.is_valid() {
            return Err(AudioDeviceAdapterError::InvalidEndpoint);
        }
        validate_permit(permit)?;
        Ok(Self {
            engine,
            endpoint_generation,
            permit,
            callback_sequence: 0,
        })
    }

    pub const fn permit(&self) -> AudioDevicePermit {
        self.permit
    }

    pub fn render_callback(
        &mut self,
        permit: AudioDevicePermit,
        mixer: &mut AudioMixerEndpoint,
        inputs: &[AudioMixInput<'_>],
        output: &mut [i16],
    ) -> Result<AudioDeviceCallbackReport, AudioDeviceAdapterError> {
        if permit != self.permit {
            return Err(AudioDeviceAdapterError::StalePermit);
        }
        if mixer.endpoint_generation() != self.endpoint_generation {
            return Err(AudioDeviceAdapterError::InvalidEndpoint);
        }
        let expected_samples = (permit.format.callback_frames as usize)
            .checked_mul(permit.format.channels as usize)
            .ok_or(AudioDeviceAdapterError::CallbackShape)?;
        if output.len() != expected_samples {
            return Err(AudioDeviceAdapterError::CallbackShape);
        }
        let callback_sequence = self
            .callback_sequence
            .checked_add(1)
            .ok_or(AudioDeviceAdapterError::CallbackSequenceExhausted)?;
        let mix = mixer.mix_stereo_into(inputs, output)?;
        self.callback_sequence = callback_sequence;
        Ok(AudioDeviceCallbackReport {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            callback_sequence,
            mix,
        })
    }

    pub fn rebind(
        &mut self,
        permit: AudioDevicePermit,
        endpoint_generation: Handle,
    ) -> Result<(), AudioDeviceAdapterError> {
        self.validate_rebind(permit, endpoint_generation)?;
        self.permit = permit;
        self.endpoint_generation = endpoint_generation;
        self.callback_sequence = 0;
        Ok(())
    }

    pub fn validate_rebind(
        &self,
        permit: AudioDevicePermit,
        endpoint_generation: Handle,
    ) -> Result<(), AudioDeviceAdapterError> {
        validate_permit(permit)?;
        if permit.lease != self.permit.lease
            || permit.device_generation.index != self.permit.device_generation.index
            || permit.device_generation.generation <= self.permit.device_generation.generation
        {
            return Err(AudioDeviceAdapterError::StalePermit);
        }
        if !endpoint_generation.is_valid() || endpoint_generation == self.endpoint_generation {
            return Err(AudioDeviceAdapterError::InvalidEndpoint);
        }
        Ok(())
    }
}

fn validate_permit(permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError> {
    if !permit.lease.is_valid()
        || !permit.realtime.is_valid()
        || !permit.device_generation.is_valid()
    {
        return Err(AudioDeviceAdapterError::InvalidPermit);
    }
    if permit.format.channels != 2
        || permit.format.sample_rate < 8_000
        || permit.format.sample_rate > 384_000
        || permit.format.callback_frames == 0
    {
        return Err(AudioDeviceAdapterError::UnsupportedFormat);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativePcm16WavDecoderConfig {
    pub max_file_bytes: usize,
    pub max_data_bytes: usize,
}

impl Default for NativePcm16WavDecoderConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024 * 1024,
            max_data_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAudioDecodeError {
    InvalidConfig,
    WrongDecodeActor,
    InvalidWork,
    FileOpen,
    FileRead,
    FileCapacity,
    MalformedWave,
    UnsupportedWave,
    ChannelMismatch,
    SampleRateMismatch,
    DecodeStartOutOfRange,
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pcm16AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub total_frames: u64,
}

pub struct NativePcm16WavDecoder {
    decode_actor: Handle,
    config: NativePcm16WavDecoderConfig,
}

impl NativePcm16WavDecoder {
    pub fn new(
        decode_actor: Handle,
        config: NativePcm16WavDecoderConfig,
    ) -> Result<Self, NativeAudioDecodeError> {
        if !decode_actor.is_valid()
            || config.max_file_bytes == 0
            || config.max_data_bytes == 0
            || config.max_data_bytes > config.max_file_bytes
        {
            return Err(NativeAudioDecodeError::InvalidConfig);
        }
        Ok(Self {
            decode_actor,
            config,
        })
    }

    pub fn decode_file(
        &self,
        work: AudioDecodeWork,
        path: &Path,
        expected_sample_rate: u32,
    ) -> Result<AudioDecodeCompletion, NativeAudioDecodeError> {
        if work.decode_actor != self.decode_actor {
            return Err(NativeAudioDecodeError::WrongDecodeActor);
        }
        if !work.engine.is_valid()
            || !work.endpoint_generation.is_valid()
            || !work.stream.handle.is_valid()
            || work.request_sequence == 0
            || work.max_frames == 0
            || work.channels == 0
            || expected_sample_rate == 0
        {
            return Err(NativeAudioDecodeError::InvalidWork);
        }
        let file = File::open(path).map_err(|_| NativeAudioDecodeError::FileOpen)?;
        let metadata_len = file
            .metadata()
            .map_err(|_| NativeAudioDecodeError::FileRead)?
            .len();
        if metadata_len > self.config.max_file_bytes as u64 {
            return Err(NativeAudioDecodeError::FileCapacity);
        }
        let limit = self
            .config
            .max_file_bytes
            .checked_add(1)
            .ok_or(NativeAudioDecodeError::FileCapacity)? as u64;
        let mut reader: Take<File> = file.take(limit);
        let metadata_len =
            usize::try_from(metadata_len).map_err(|_| NativeAudioDecodeError::FileCapacity)?;
        let mut bytes = Vec::with_capacity(metadata_len);
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| NativeAudioDecodeError::FileRead)?;
        if bytes.len() > self.config.max_file_bytes {
            return Err(NativeAudioDecodeError::FileCapacity);
        }
        self.decode_bytes(work, &bytes, expected_sample_rate)
    }

    pub fn inspect_bytes(&self, bytes: &[u8]) -> Result<Pcm16AudioFormat, NativeAudioDecodeError> {
        if bytes.len() > self.config.max_file_bytes {
            return Err(NativeAudioDecodeError::FileCapacity);
        }
        let wave = parse_pcm16_wave(bytes, self.config.max_data_bytes)?;
        let frame_bytes = usize::from(wave.channels)
            .checked_mul(2)
            .ok_or(NativeAudioDecodeError::ArithmeticOverflow)?;
        Ok(Pcm16AudioFormat {
            sample_rate: wave.sample_rate,
            channels: wave.channels,
            total_frames: u64::try_from(wave.data.len() / frame_bytes)
                .map_err(|_| NativeAudioDecodeError::ArithmeticOverflow)?,
        })
    }

    pub fn decode_bytes(
        &self,
        work: AudioDecodeWork,
        bytes: &[u8],
        expected_sample_rate: u32,
    ) -> Result<AudioDecodeCompletion, NativeAudioDecodeError> {
        if work.decode_actor != self.decode_actor {
            return Err(NativeAudioDecodeError::WrongDecodeActor);
        }
        if !work.engine.is_valid()
            || !work.endpoint_generation.is_valid()
            || !work.stream.handle.is_valid()
            || work.request_sequence == 0
            || work.max_frames == 0
            || work.channels == 0
            || expected_sample_rate == 0
        {
            return Err(NativeAudioDecodeError::InvalidWork);
        }
        if bytes.len() > self.config.max_file_bytes {
            return Err(NativeAudioDecodeError::FileCapacity);
        }
        let wave = parse_pcm16_wave(bytes, self.config.max_data_bytes)?;
        if wave.channels != work.channels {
            return Err(NativeAudioDecodeError::ChannelMismatch);
        }
        if wave.sample_rate != expected_sample_rate {
            return Err(NativeAudioDecodeError::SampleRateMismatch);
        }
        let channels = work.channels as usize;
        let total_frames = wave.data.len() / (channels * 2);
        let start = usize::try_from(work.start_frame)
            .map_err(|_| NativeAudioDecodeError::DecodeStartOutOfRange)?;
        if start > total_frames {
            return Err(NativeAudioDecodeError::DecodeStartOutOfRange);
        }
        let frames = work.max_frames.min(total_frames - start);
        let start_byte = start
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(NativeAudioDecodeError::ArithmeticOverflow)?;
        let byte_len = frames
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(NativeAudioDecodeError::ArithmeticOverflow)?;
        let end_byte = start_byte
            .checked_add(byte_len)
            .ok_or(NativeAudioDecodeError::ArithmeticOverflow)?;
        let pcm = wave
            .data
            .get(start_byte..end_byte)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        let mut samples = Vec::with_capacity(frames * channels);
        for encoded in pcm.chunks_exact(2) {
            samples.push(i16::from_le_bytes([encoded[0], encoded[1]]));
        }
        Ok(AudioDecodeCompletion {
            engine: work.engine,
            endpoint_generation: work.endpoint_generation,
            decode_actor: work.decode_actor,
            stream: work.stream,
            request_sequence: work.request_sequence,
            start_frame: work.start_frame,
            channels: work.channels,
            samples,
            end_of_stream: start + frames == total_frames,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioAssetStoreConfig {
    pub max_assets: usize,
    pub max_total_bytes: usize,
    pub decoder: NativePcm16WavDecoderConfig,
}

impl Default for AudioAssetStoreConfig {
    fn default() -> Self {
        Self {
            max_assets: 1024,
            max_total_bytes: 512 * 1024 * 1024,
            decoder: NativePcm16WavDecoderConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioAssetStoreError {
    InvalidConfig,
    WrongEngine,
    InvalidAsset,
    AssetCapacity,
    ByteCapacity,
    UnknownAsset,
    Decode(NativeAudioDecodeError),
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioAssetStoreOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub resident_assets: usize,
    pub resident_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioAssetStoreShutdownReport {
    pub released_assets: usize,
    pub released_bytes: usize,
}

pub struct AudioAssetStore {
    engine: EngineId,
    decoder: NativePcm16WavDecoder,
    config: AudioAssetStoreConfig,
    assets: BTreeMap<AssetRef, Arc<[u8]>>,
    formats: BTreeMap<AssetRef, Pcm16AudioFormat>,
    total_bytes: usize,
    closed: bool,
}

impl AudioAssetStore {
    pub fn new(
        engine: EngineId,
        decode_actor: Handle,
        config: AudioAssetStoreConfig,
    ) -> Result<Self, AudioAssetStoreError> {
        if !engine.is_valid() || config.max_assets == 0 || config.max_total_bytes == 0 {
            return Err(AudioAssetStoreError::InvalidConfig);
        }
        Ok(Self {
            engine,
            decoder: NativePcm16WavDecoder::new(decode_actor, config.decoder)
                .map_err(AudioAssetStoreError::Decode)?,
            config,
            assets: BTreeMap::new(),
            formats: BTreeMap::new(),
            total_bytes: 0,
            closed: false,
        })
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn asset_count(&self) -> usize {
        self.assets.len()
    }

    pub fn owner_snapshot(&self) -> AudioAssetStoreOwnerSnapshot {
        AudioAssetStoreOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            resident_assets: self.assets.len(),
            resident_bytes: self.total_bytes,
        }
    }

    pub fn contains(&self, asset: AssetRef) -> bool {
        self.assets.contains_key(&asset)
    }

    pub fn format(&self, asset: AssetRef) -> Result<Pcm16AudioFormat, AudioAssetStoreError> {
        self.ensure_open()?;
        self.formats
            .get(&asset)
            .copied()
            .ok_or(AudioAssetStoreError::UnknownAsset)
    }

    pub fn insert(
        &mut self,
        asset: AssetRef,
        bytes: Arc<[u8]>,
    ) -> Result<Pcm16AudioFormat, AudioAssetStoreError> {
        self.ensure_open()?;
        self.validate_asset(asset)?;
        if !self.assets.contains_key(&asset) && self.assets.len() == self.config.max_assets {
            return Err(AudioAssetStoreError::AssetCapacity);
        }
        let replaced = self.assets.get(&asset).map_or(0, |bytes| bytes.len());
        let total_bytes = self
            .total_bytes
            .checked_sub(replaced)
            .and_then(|total| total.checked_add(bytes.len()))
            .filter(|total| *total <= self.config.max_total_bytes)
            .ok_or(AudioAssetStoreError::ByteCapacity)?;
        let format = self
            .decoder
            .inspect_bytes(&bytes)
            .map_err(AudioAssetStoreError::Decode)?;
        self.assets.insert(asset, bytes);
        self.formats.insert(asset, format);
        self.total_bytes = total_bytes;
        Ok(format)
    }

    pub fn remove(&mut self, asset: AssetRef) -> Result<bool, AudioAssetStoreError> {
        self.ensure_open()?;
        self.validate_asset(asset)?;
        let Some(bytes) = self.assets.remove(&asset) else {
            return Ok(false);
        };
        self.formats.remove(&asset);
        self.total_bytes -= bytes.len();
        Ok(true)
    }

    pub fn decode(
        &self,
        asset: AssetRef,
        work: AudioDecodeWork,
    ) -> Result<AudioDecodeCompletion, AudioAssetStoreError> {
        self.ensure_open()?;
        self.validate_asset(asset)?;
        let bytes = self
            .assets
            .get(&asset)
            .ok_or(AudioAssetStoreError::UnknownAsset)?;
        let format = self.format(asset)?;
        self.decoder
            .decode_bytes(work, bytes, format.sample_rate)
            .map_err(AudioAssetStoreError::Decode)
    }

    pub fn read_frames(
        &self,
        asset: AssetRef,
        start_frame: u64,
        max_frames: usize,
    ) -> Result<(Vec<i16>, bool), AudioAssetStoreError> {
        self.ensure_open()?;
        self.validate_asset(asset)?;
        let bytes = self
            .assets
            .get(&asset)
            .ok_or(AudioAssetStoreError::UnknownAsset)?;
        let wave = parse_pcm16_wave(bytes, self.config.decoder.max_data_bytes)
            .map_err(AudioAssetStoreError::Decode)?;
        let channels = usize::from(wave.channels);
        let total_frames = wave.data.len() / (channels * 2);
        let start = usize::try_from(start_frame).map_err(|_| {
            AudioAssetStoreError::Decode(NativeAudioDecodeError::DecodeStartOutOfRange)
        })?;
        if start > total_frames {
            return Err(AudioAssetStoreError::Decode(
                NativeAudioDecodeError::DecodeStartOutOfRange,
            ));
        }
        let frames = max_frames.min(total_frames - start);
        let start_byte = start
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(AudioAssetStoreError::Decode(
                NativeAudioDecodeError::ArithmeticOverflow,
            ))?;
        let end_byte = start_byte
            .checked_add(
                frames
                    .checked_mul(channels)
                    .and_then(|samples| samples.checked_mul(2))
                    .ok_or(AudioAssetStoreError::Decode(
                        NativeAudioDecodeError::ArithmeticOverflow,
                    ))?,
            )
            .ok_or(AudioAssetStoreError::Decode(
                NativeAudioDecodeError::ArithmeticOverflow,
            ))?;
        let mut samples = Vec::with_capacity(frames * channels);
        for encoded in wave.data[start_byte..end_byte].chunks_exact(2) {
            samples.push(i16::from_le_bytes([encoded[0], encoded[1]]));
        }
        Ok((samples, start + frames == total_frames))
    }

    pub fn shutdown(&mut self) -> AudioAssetStoreShutdownReport {
        if self.closed {
            return AudioAssetStoreShutdownReport {
                released_assets: 0,
                released_bytes: 0,
            };
        }
        let report = AudioAssetStoreShutdownReport {
            released_assets: self.assets.len(),
            released_bytes: self.total_bytes,
        };
        self.assets.clear();
        self.formats.clear();
        self.total_bytes = 0;
        self.closed = true;
        report
    }

    fn ensure_open(&self) -> Result<(), AudioAssetStoreError> {
        if self.closed {
            Err(AudioAssetStoreError::Closed)
        } else {
            Ok(())
        }
    }

    fn validate_asset(&self, asset: AssetRef) -> Result<(), AudioAssetStoreError> {
        if asset.engine != self.engine {
            return Err(AudioAssetStoreError::WrongEngine);
        }
        if !asset.handle.is_valid() {
            return Err(AudioAssetStoreError::InvalidAsset);
        }
        Ok(())
    }
}

struct ParsedWave<'a> {
    channels: u16,
    sample_rate: u32,
    data: &'a [u8],
}

fn parse_pcm16_wave(
    bytes: &[u8],
    max_data_bytes: usize,
) -> Result<ParsedWave<'_>, NativeAudioDecodeError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(NativeAudioDecodeError::MalformedWave);
    }
    let declared = read_u32(bytes, 4)? as usize;
    if declared
        .checked_add(8)
        .filter(|declared| *declared == bytes.len())
        .is_none()
    {
        return Err(NativeAudioDecodeError::MalformedWave);
    }
    let mut offset = 12_usize;
    let mut format = None;
    let mut data = None;
    while offset < bytes.len() {
        let header_end = offset
            .checked_add(8)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let chunk_start = header_end;
        let chunk_end = chunk_start
            .checked_add(size)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        let chunk = bytes
            .get(chunk_start..chunk_end)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        match &header[0..4] {
            b"fmt " => {
                if format.is_some() || chunk.len() < 16 {
                    return Err(NativeAudioDecodeError::MalformedWave);
                }
                let encoding = u16::from_le_bytes([chunk[0], chunk[1]]);
                let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
                let sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                let byte_rate = u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
                let block_align = u16::from_le_bytes([chunk[12], chunk[13]]);
                let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
                let expected_align = channels
                    .checked_mul(2)
                    .ok_or(NativeAudioDecodeError::MalformedWave)?;
                let expected_rate = sample_rate
                    .checked_mul(expected_align as u32)
                    .ok_or(NativeAudioDecodeError::MalformedWave)?;
                if encoding != 1
                    || channels == 0
                    || sample_rate == 0
                    || bits != 16
                    || block_align != expected_align
                    || byte_rate != expected_rate
                {
                    return Err(NativeAudioDecodeError::UnsupportedWave);
                }
                format = Some((channels, sample_rate, block_align as usize));
            }
            b"data" => {
                if data.is_some() || size > max_data_bytes {
                    return Err(if size > max_data_bytes {
                        NativeAudioDecodeError::FileCapacity
                    } else {
                        NativeAudioDecodeError::MalformedWave
                    });
                }
                data = Some(chunk);
            }
            _ => {}
        }
        offset = chunk_end
            .checked_add(size & 1)
            .ok_or(NativeAudioDecodeError::MalformedWave)?;
        if offset > bytes.len() {
            return Err(NativeAudioDecodeError::MalformedWave);
        }
    }
    if offset != bytes.len() {
        return Err(NativeAudioDecodeError::MalformedWave);
    }
    let (channels, sample_rate, block_align) =
        format.ok_or(NativeAudioDecodeError::MalformedWave)?;
    let data = data.ok_or(NativeAudioDecodeError::MalformedWave)?;
    if data.len() % block_align != 0 {
        return Err(NativeAudioDecodeError::MalformedWave);
    }
    Ok(ParsedWave {
        channels,
        sample_rate,
        data,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, NativeAudioDecodeError> {
    let end = offset
        .checked_add(4)
        .ok_or(NativeAudioDecodeError::MalformedWave)?;
    let bytes = bytes
        .get(offset..end)
        .ok_or(NativeAudioDecodeError::MalformedWave)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use super::*;
    use crate::{
        asset::AssetRef,
        audio::PersistentSourceRecoveryPolicy,
        audio_mixer::{
            AudioBusDescriptor, AudioControlState, AudioListener, AudioMixerConfig,
            PersistentAudioSourceDescriptor,
        },
        audio_streaming::{AudioStreamingConfig, AudioStreamingEndpoint},
        control::{ControlKind, StableControlRef},
    };
    use vo_app_protocol::{
        AudioDeviceFormat, AudioDeviceGeneration, AudioDeviceLeaseHandle, AudioRealtimeEndpoint,
        GenerationalHandle, SessionHandle,
    };

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn stable(kind: ControlKind, index: u32) -> StableControlRef {
        StableControlRef {
            engine: handle(1),
            kind,
            handle: handle(index),
        }
    }

    fn source_control() -> PersistentAudioSourceDescriptor {
        PersistentAudioSourceDescriptor {
            source: stable(ControlKind::PersistentAudioSource, 2),
            bus: stable(ControlKind::AudioBus, 1),
            asset: AssetRef {
                engine: handle(1),
                handle: handle(3),
            },
            gain_q15: 32_767,
            spatial: None,
            looped: false,
            recovery: PersistentSourceRecoveryPolicy::ResumeTimeline,
            transport_anchor_millis: 0,
        }
    }

    fn new_mixer(generation: Handle) -> AudioMixerEndpoint {
        let mut mixer = AudioMixerEndpoint::new(
            handle(1),
            generation,
            AudioMixerConfig {
                max_callback_frames: 2,
                ..AudioMixerConfig::default()
            },
        )
        .expect("mixer");
        mixer
            .apply_control_state(AudioControlState {
                engine: handle(1),
                revision: 1,
                buses: vec![AudioBusDescriptor {
                    bus: stable(ControlKind::AudioBus, 1),
                    parent: None,
                    gain_q15: 32_767,
                    mute: false,
                    solo: false,
                }],
                sources: vec![source_control()],
                ducking: Vec::new(),
                listener: AudioListener::default(),
            })
            .expect("control");
        mixer
    }

    fn permit() -> AudioDevicePermit {
        AudioDevicePermit {
            lease: AudioDeviceLeaseHandle {
                index: 1,
                generation: 1,
            },
            realtime: AudioRealtimeEndpoint {
                session: SessionHandle {
                    index: 2,
                    generation: 1,
                },
                session_epoch: 3,
                endpoint: GenerationalHandle {
                    index: 4,
                    generation: 1,
                },
                endpoint_epoch: 5,
            },
            device_generation: AudioDeviceGeneration {
                index: 1,
                generation: 1,
            },
            format: AudioDeviceFormat {
                sample_rate: 8_000,
                channels: 2,
                callback_frames: 2,
            },
        }
    }

    fn wave(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    fn temp_wave(bytes: &[u8]) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("voplay-audio-{}-{nonce}.wav", std::process::id()));
        fs::write(&path, bytes).expect("write wave");
        path
    }

    #[test]
    fn native_wave_decode_ring_mixer_and_lease_callback_match_reference() {
        let path = temp_wave(&wave(&[1000, -1000], 8_000, 1));
        let source = source_control();
        let mut streaming = AudioStreamingEndpoint::new(
            handle(1),
            handle(50),
            handle(60),
            AudioStreamingConfig {
                max_callback_frames: 2,
                ..AudioStreamingConfig::default()
            },
        )
        .expect("streaming");
        let stream = streaming
            .create_stream_from_control(&source, 8_000, 1, 2)
            .expect("stream");
        let work = streaming.request_decode(stream).expect("decode work");
        let decoder =
            NativePcm16WavDecoder::new(handle(60), NativePcm16WavDecoderConfig::default())
                .expect("decoder");
        let completion = decoder
            .decode_file(work, &path, 8_000)
            .expect("decode file");
        assert!(completion.end_of_stream);
        streaming
            .complete_decode(completion)
            .expect("complete decode");
        let mut mono = [0_i16; 2];
        streaming
            .render_into(stream, &mut mono)
            .expect("stream callback");
        assert_eq!(mono, [1000, -1000]);

        let permit = permit();
        let mut mixer = new_mixer(handle(50));
        let voice = mixer.start_persistent(source.source).expect("voice");
        let inputs = [AudioMixInput {
            voice,
            channels: 1,
            samples: &mono,
        }];
        let mut adapter =
            AppAudioDeviceAdapter::new(handle(1), handle(50), permit).expect("adapter");
        let mut output = [0_i16; 4];
        let report = adapter
            .render_callback(permit, &mut mixer, &inputs, &mut output)
            .expect("device callback");
        assert_eq!(output, [1000, 1000, -1000, -1000]);
        assert_eq!(report.callback_sequence, 1);

        let mut reference_mixer = new_mixer(handle(50));
        let reference_voice = reference_mixer
            .start_persistent(source.source)
            .expect("reference voice");
        let reference_inputs = [AudioMixInput {
            voice: reference_voice,
            channels: 1,
            samples: &mono,
        }];
        let mut reference = [0_i16; 4];
        reference_mixer
            .mix_stereo_into(&reference_inputs, &mut reference)
            .expect("reference mix");
        assert_eq!(output, reference);
        fs::remove_file(path).expect("remove wave");
    }

    #[test]
    fn malformed_oversized_and_wrong_shape_wave_fail_before_pcm_completion() {
        let malformed = temp_wave(b"not-wave");
        let source = source_control();
        let mut streaming = AudioStreamingEndpoint::new(
            handle(1),
            handle(50),
            handle(60),
            AudioStreamingConfig::default(),
        )
        .expect("streaming");
        let stream = streaming
            .create_stream_from_control(&source, 8_000, 1, 2)
            .expect("stream");
        let work = streaming.request_decode(stream).expect("work");
        let decoder = NativePcm16WavDecoder::new(
            handle(60),
            NativePcm16WavDecoderConfig {
                max_file_bytes: 64,
                max_data_bytes: 32,
            },
        )
        .expect("decoder");
        assert_eq!(
            decoder.decode_file(work, &malformed, 8_000),
            Err(NativeAudioDecodeError::MalformedWave)
        );
        assert_eq!(streaming.buffered_frames(stream), Ok(0));

        let stereo = temp_wave(&wave(&[1, 2, 3, 4], 8_000, 2));
        assert_eq!(
            decoder.decode_file(work, &stereo, 8_000),
            Err(NativeAudioDecodeError::ChannelMismatch)
        );
        let oversized = temp_wave(&wave(&[0; 20], 8_000, 1));
        assert_eq!(
            decoder.decode_file(work, &oversized, 8_000),
            Err(NativeAudioDecodeError::FileCapacity)
        );
        for path in [malformed, stereo, oversized] {
            fs::remove_file(path).expect("remove fixture");
        }
    }

    #[test]
    fn stale_permit_and_rebind_generation_are_enforced_before_output() {
        let permit = permit();
        let mut adapter =
            AppAudioDeviceAdapter::new(handle(1), handle(50), permit).expect("adapter");
        let mut mixer = new_mixer(handle(50));
        let voice = mixer
            .start_persistent(source_control().source)
            .expect("voice");
        let samples = [1_i16, 2];
        let inputs = [AudioMixInput {
            voice,
            channels: 1,
            samples: &samples,
        }];
        let mut forged = permit;
        forged.device_generation.generation += 1;
        let mut output = [77_i16; 4];
        assert_eq!(
            adapter.render_callback(forged, &mut mixer, &inputs, &mut output),
            Err(AudioDeviceAdapterError::StalePermit)
        );
        assert_eq!(output, [77; 4]);

        let mut recovered_permit = permit;
        recovered_permit.device_generation.generation += 1;
        recovered_permit.realtime.endpoint.generation += 1;
        recovered_permit.realtime.endpoint_epoch += 1;
        mixer.restart().expect("mixer restart");
        adapter
            .rebind(recovered_permit, mixer.endpoint_generation())
            .expect("adapter rebind");
        let recovered_voice = mixer
            .start_persistent(source_control().source)
            .expect("recovered voice");
        let recovered_inputs = [AudioMixInput {
            voice: recovered_voice,
            channels: 1,
            samples: &samples,
        }];
        output.fill(0);
        assert_eq!(
            adapter
                .render_callback(recovered_permit, &mut mixer, &recovered_inputs, &mut output,)
                .expect("recovered callback")
                .callback_sequence,
            1
        );
        assert_eq!(output, [1, 1, 2, 2]);
    }
}
