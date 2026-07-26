use std::sync::{
    atomic::{AtomicI16, AtomicU64, AtomicUsize, Ordering},
    mpsc::{sync_channel, Receiver, SyncSender},
    Arc,
};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use vo_app_protocol::AudioDevicePermit;
use voplay_protocol::{EngineId, Handle};
use voplay_runtime::{
    audio::AudioEvent,
    audio_device_adapter::AudioDeviceAdapterError,
    audio_endpoint::{AudioEventExecution, AudioEventExecutor, MixerAudioEventExecutor},
    audio_mixer::AudioMixerEndpoint,
};

use crate::{NativeAudioDevice, NativeAudioOwner};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealtimeRingError {
    InvalidCapacity,
    ProducerOverflow,
    WorkerUnavailable,
}

pub struct RealtimePcmRing {
    samples: Box<[AtomicI16]>,
    read: AtomicUsize,
    write: AtomicUsize,
    underflow_samples: AtomicU64,
}

impl RealtimePcmRing {
    pub fn new(capacity_samples: usize) -> Result<Self, RealtimeRingError> {
        if capacity_samples < 2 {
            return Err(RealtimeRingError::InvalidCapacity);
        }
        let samples = (0..capacity_samples)
            .map(|_| AtomicI16::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(Self {
            samples,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            underflow_samples: AtomicU64::new(0),
        })
    }

    pub fn push(&self, input: &[i16]) -> Result<(), RealtimeRingError> {
        let capacity = self.samples.len();
        let read = self.read.load(Ordering::Acquire);
        let write = self.write.load(Ordering::Relaxed);
        let available = if write >= read {
            capacity - (write - read) - 1
        } else {
            read - write - 1
        };
        if input.len() > available {
            return Err(RealtimeRingError::ProducerOverflow);
        }
        let mut cursor = write;
        for sample in input {
            self.samples[cursor].store(*sample, Ordering::Relaxed);
            cursor += 1;
            if cursor == capacity {
                cursor = 0;
            }
        }
        self.write.store(cursor, Ordering::Release);
        Ok(())
    }

    pub fn pop_realtime(&self, output: &mut [i16]) -> usize {
        self.pop_realtime_convert(output, 0, |sample| sample)
    }

    fn pop_realtime_f32(&self, output: &mut [f32]) -> usize {
        self.pop_realtime_convert(output, 0.0, |sample| f32::from(sample) / 32_768.0)
    }

    fn pop_realtime_u16(&self, output: &mut [u16]) -> usize {
        self.pop_realtime_convert(output, 32_768, |sample| (i32::from(sample) + 32_768) as u16)
    }

    fn pop_realtime_convert<T: Copy>(
        &self,
        output: &mut [T],
        silence: T,
        convert: impl Fn(i16) -> T,
    ) -> usize {
        let capacity = self.samples.len();
        let mut read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let available = if write >= read {
            write - read
        } else {
            capacity - read + write
        };
        let copied = available.min(output.len());
        for sample in &mut output[..copied] {
            *sample = convert(self.samples[read].load(Ordering::Relaxed));
            read += 1;
            if read == capacity {
                read = 0;
            }
        }
        output[copied..].fill(silence);
        self.read.store(read, Ordering::Release);
        if copied < output.len() {
            self.underflow_samples
                .fetch_add((output.len() - copied) as u64, Ordering::Relaxed);
        }
        copied
    }

    pub fn underflow_samples(&self) -> u64 {
        self.underflow_samples.load(Ordering::Relaxed)
    }
}

pub struct CpalAudioDevice {
    ring: Arc<RealtimePcmRing>,
    stream: Option<cpal::Stream>,
    callback_invocations: Arc<AtomicU64>,
    callback_errors: Arc<AtomicU64>,
}

impl CpalAudioDevice {
    pub fn new(ring: Arc<RealtimePcmRing>) -> Self {
        Self {
            ring,
            stream: None,
            callback_invocations: Arc::new(AtomicU64::new(0)),
            callback_errors: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn ring(&self) -> &Arc<RealtimePcmRing> {
        &self.ring
    }

    pub fn callback_error_count(&self) -> u64 {
        self.callback_errors.load(Ordering::Relaxed)
    }

    pub fn callback_invocation_count(&self) -> u64 {
        self.callback_invocations.load(Ordering::Relaxed)
    }
}

impl NativeAudioDevice for CpalAudioDevice {
    fn activate(&mut self, permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError> {
        if self.stream.is_some() {
            return Err(AudioDeviceAdapterError::StalePermit);
        }
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioDeviceAdapterError::UnsupportedFormat)?;
        let sample_rate = cpal::SampleRate(permit.format.sample_rate);
        let supported = device
            .supported_output_configs()
            .map_err(|_| AudioDeviceAdapterError::UnsupportedFormat)?
            .filter(|config| {
                config.channels() == permit.format.channels
                    && config.min_sample_rate() <= sample_rate
                    && config.max_sample_rate() >= sample_rate
            })
            .min_by_key(|config| match config.sample_format() {
                cpal::SampleFormat::I16 => 0,
                cpal::SampleFormat::F32 => 1,
                cpal::SampleFormat::U16 => 2,
                _ => 3,
            })
            .ok_or(AudioDeviceAdapterError::UnsupportedFormat)?;
        let sample_format = supported.sample_format();
        let config = cpal::StreamConfig {
            channels: permit.format.channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Fixed(permit.format.callback_frames),
        };
        let stream = match sample_format {
            cpal::SampleFormat::I16 => {
                let ring = Arc::clone(&self.ring);
                let callback_invocations = Arc::clone(&self.callback_invocations);
                let callback_errors = Arc::clone(&self.callback_errors);
                device.build_output_stream(
                    &config,
                    move |output: &mut [i16], _| {
                        ring.pop_realtime(output);
                        callback_invocations.fetch_add(1, Ordering::Relaxed);
                    },
                    move |_| {
                        callback_errors.fetch_add(1, Ordering::Relaxed);
                    },
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let ring = Arc::clone(&self.ring);
                let callback_invocations = Arc::clone(&self.callback_invocations);
                let callback_errors = Arc::clone(&self.callback_errors);
                device.build_output_stream(
                    &config,
                    move |output: &mut [f32], _| {
                        ring.pop_realtime_f32(output);
                        callback_invocations.fetch_add(1, Ordering::Relaxed);
                    },
                    move |_| {
                        callback_errors.fetch_add(1, Ordering::Relaxed);
                    },
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let ring = Arc::clone(&self.ring);
                let callback_invocations = Arc::clone(&self.callback_invocations);
                let callback_errors = Arc::clone(&self.callback_errors);
                device.build_output_stream(
                    &config,
                    move |output: &mut [u16], _| {
                        ring.pop_realtime_u16(output);
                        callback_invocations.fetch_add(1, Ordering::Relaxed);
                    },
                    move |_| {
                        callback_errors.fetch_add(1, Ordering::Relaxed);
                    },
                    None,
                )
            }
            _ => return Err(AudioDeviceAdapterError::UnsupportedFormat),
        }
        .map_err(|_| AudioDeviceAdapterError::UnsupportedFormat)?;
        stream
            .play()
            .map_err(|_| AudioDeviceAdapterError::UnsupportedFormat)?;
        self.stream = Some(stream);
        Ok(())
    }

    fn deactivate(&mut self, _permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError> {
        self.stream.take();
        Ok(())
    }
}

pub struct CpalAudioEventExecutor {
    ring: Arc<RealtimePcmRing>,
    control: CpalControlWorker,
    mixer_executor: MixerAudioEventExecutor,
}

impl CpalAudioEventExecutor {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        ring_capacity_samples: usize,
    ) -> Result<Self, RealtimeRingError> {
        let ring = Arc::new(RealtimePcmRing::new(ring_capacity_samples)?);
        Ok(Self {
            control: CpalControlWorker::new(engine, endpoint_generation, Arc::clone(&ring))?,
            ring,
            mixer_executor: MixerAudioEventExecutor,
        })
    }

    pub fn submit_pcm(&self, samples: &[i16]) -> Result<(), RealtimeRingError> {
        self.ring.push(samples)
    }

    pub fn underflow_samples(&self) -> u64 {
        self.ring.underflow_samples()
    }
}

impl AudioEventExecutor for CpalAudioEventExecutor {
    fn activate(&mut self, permit: AudioDevicePermit) -> Result<(), ()> {
        self.control.request(CpalControlOperation::Activate(permit))
    }

    fn suspend(&mut self) -> Result<(), ()> {
        self.control.request(CpalControlOperation::Suspend)
    }

    fn resume(&mut self, permit: AudioDevicePermit) -> Result<(), ()> {
        self.control.request(CpalControlOperation::Resume(permit))
    }

    fn device_lost(&mut self) -> Result<(), ()> {
        self.control.request(CpalControlOperation::DeviceLost)
    }

    fn submit_pcm(&mut self, samples: &[i16]) -> Result<(), ()> {
        self.ring.push(samples).map_err(|_| ())
    }

    fn execute(
        &mut self,
        event: &AudioEvent,
        mixer: &mut AudioMixerEndpoint,
    ) -> AudioEventExecution {
        self.mixer_executor.execute(event, mixer)
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), ()> {
        self.control.request(CpalControlOperation::Close)
    }
}

enum CpalControlOperation {
    Activate(AudioDevicePermit),
    Suspend,
    Resume(AudioDevicePermit),
    DeviceLost,
    Close,
}

enum CpalControlMessage {
    Operation {
        operation: CpalControlOperation,
        reply: SyncSender<Result<(), ()>>,
    },
    Stop,
}

struct CpalControlWorker {
    sender: SyncSender<CpalControlMessage>,
    join: Option<JoinHandle<()>>,
}

impl CpalControlWorker {
    fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        ring: Arc<RealtimePcmRing>,
    ) -> Result<Self, RealtimeRingError> {
        let (sender, receiver) = sync_channel(16);
        let join = std::thread::Builder::new()
            .name(String::from("voplay-cpal-owner"))
            .spawn(move || run_cpal_control(engine, endpoint_generation, ring, receiver))
            .map_err(|_| RealtimeRingError::WorkerUnavailable)?;
        Ok(Self {
            sender,
            join: Some(join),
        })
    }

    fn request(&self, operation: CpalControlOperation) -> Result<(), ()> {
        let (reply, result) = sync_channel(1);
        self.sender
            .send(CpalControlMessage::Operation { operation, reply })
            .map_err(|_| ())?;
        result.recv().map_err(|_| ())?
    }
}

impl Drop for CpalControlWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(CpalControlMessage::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_cpal_control(
    engine: EngineId,
    endpoint_generation: Handle,
    ring: Arc<RealtimePcmRing>,
    receiver: Receiver<CpalControlMessage>,
) {
    let mut owner: Option<NativeAudioOwner<CpalAudioDevice>> = None;
    while let Ok(message) = receiver.recv() {
        match message {
            CpalControlMessage::Operation { operation, reply } => {
                let result = match operation {
                    CpalControlOperation::Activate(permit)
                    | CpalControlOperation::Resume(permit) => {
                        replace_cpal_device(&mut owner, engine, endpoint_generation, permit, &ring)
                    }
                    CpalControlOperation::Suspend | CpalControlOperation::Close => {
                        close_cpal_device(&mut owner)
                    }
                    CpalControlOperation::DeviceLost => {
                        if let Some(mut current) = owner.take() {
                            let _ = current.close();
                        }
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            CpalControlMessage::Stop => {
                if let Some(mut current) = owner.take() {
                    let _ = current.close();
                }
                break;
            }
        }
    }
}

fn replace_cpal_device(
    owner: &mut Option<NativeAudioOwner<CpalAudioDevice>>,
    engine: EngineId,
    endpoint_generation: Handle,
    permit: AudioDevicePermit,
    ring: &Arc<RealtimePcmRing>,
) -> Result<(), ()> {
    close_cpal_device(owner)?;
    let device = CpalAudioDevice::new(Arc::clone(ring));
    *owner =
        Some(NativeAudioOwner::new(engine, endpoint_generation, permit, device).map_err(|_| ())?);
    Ok(())
}

fn close_cpal_device(owner: &mut Option<NativeAudioOwner<CpalAudioDevice>>) -> Result<(), ()> {
    if let Some(mut current) = owner.take() {
        current.close().map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_ring_wraps_without_overwriting_and_zero_fills_underflow() {
        let ring = RealtimePcmRing::new(5).unwrap();
        ring.push(&[1, 2, 3, 4]).unwrap();
        assert_eq!(ring.push(&[5]), Err(RealtimeRingError::ProducerOverflow));
        let mut first = [0; 3];
        assert_eq!(ring.pop_realtime(&mut first), 3);
        assert_eq!(first, [1, 2, 3]);

        ring.push(&[5, 6, 7]).unwrap();
        let mut second = [9; 6];
        assert_eq!(ring.pop_realtime(&mut second), 4);
        assert_eq!(second, [4, 5, 6, 7, 0, 0]);
        assert_eq!(ring.underflow_samples(), 2);
    }

    #[test]
    fn cpal_executor_control_handle_is_send_without_moving_platform_stream() {
        fn assert_send<T: Send>() {}
        assert_send::<CpalAudioEventExecutor>();
    }
}
