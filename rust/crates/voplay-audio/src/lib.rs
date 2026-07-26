use vo_app_protocol::AudioDevicePermit;
use voplay_protocol::{EngineId, Handle};
use voplay_runtime::{
    audio_device_adapter::{
        AppAudioDeviceAdapter, AudioDeviceAdapterError, AudioDeviceCallbackReport,
    },
    audio_mixer::{AudioMixInput, AudioMixerEndpoint},
};

#[cfg(feature = "native-cpal")]
mod cpal_device;
#[cfg(feature = "native-cpal")]
pub use cpal_device::{
    CpalAudioDevice, CpalAudioEventExecutor, RealtimePcmRing, RealtimeRingError,
};

pub trait NativeAudioDevice {
    fn activate(&mut self, permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError>;
    fn deactivate(&mut self, permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError>;
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

pub struct NativeAudioOwner<D> {
    device: D,
    callback: AppAudioDeviceAdapter,
    active: bool,
}

impl<D: NativeAudioDevice> NativeAudioOwner<D> {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        permit: AudioDevicePermit,
        mut device: D,
    ) -> Result<Self, AudioDeviceAdapterError> {
        let callback = AppAudioDeviceAdapter::new(engine, endpoint_generation, permit)?;
        device.activate(permit)?;
        Ok(Self {
            device,
            callback,
            active: true,
        })
    }

    pub fn render_callback(
        &mut self,
        permit: AudioDevicePermit,
        mixer: &mut AudioMixerEndpoint,
        inputs: &[AudioMixInput<'_>],
        output: &mut [i16],
    ) -> Result<AudioDeviceCallbackReport, AudioDeviceAdapterError> {
        if !self.active {
            return Err(AudioDeviceAdapterError::StalePermit);
        }
        self.callback.render_callback(permit, mixer, inputs, output)
    }

    pub fn rebind(
        &mut self,
        permit: AudioDevicePermit,
        endpoint_generation: Handle,
    ) -> Result<(), AudioDeviceAdapterError> {
        self.callback.validate_rebind(permit, endpoint_generation)?;
        if self.active {
            self.device.deactivate(self.callback.permit())?;
            self.active = false;
        }
        self.callback.rebind(permit, endpoint_generation)?;
        self.device.activate(permit)?;
        self.active = true;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), AudioDeviceAdapterError> {
        if self.active {
            self.device.deactivate(self.callback.permit())?;
            self.active = false;
        }
        Ok(())
    }

    pub fn device(&self) -> &D {
        &self.device
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vo_app_protocol::{
        AudioDeviceFormat, AudioDevicePermit, AudioRealtimeEndpoint, GenerationalHandle,
    };

    #[derive(Default)]
    struct RecordingDevice {
        activated: Vec<AudioDevicePermit>,
        deactivated: Vec<AudioDevicePermit>,
    }

    impl NativeAudioDevice for RecordingDevice {
        fn activate(&mut self, permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError> {
            self.activated.push(permit);
            Ok(())
        }

        fn deactivate(&mut self, permit: AudioDevicePermit) -> Result<(), AudioDeviceAdapterError> {
            self.deactivated.push(permit);
            Ok(())
        }
    }

    fn app_handle(index: u32, generation: u32) -> GenerationalHandle {
        GenerationalHandle { index, generation }
    }

    fn engine_handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn permit(device_generation: u32) -> AudioDevicePermit {
        AudioDevicePermit {
            lease: app_handle(1, 1),
            realtime: AudioRealtimeEndpoint {
                session: app_handle(2, 1),
                session_epoch: 3,
                endpoint: app_handle(4, 1),
                endpoint_epoch: 5,
            },
            device_generation: app_handle(6, device_generation),
            format: AudioDeviceFormat {
                sample_rate: 48_000,
                channels: 2,
                callback_frames: 128,
            },
        }
    }

    #[test]
    fn stale_rebind_is_rejected_before_deactivating_live_device() {
        let initial = permit(1);
        let mut owner = NativeAudioOwner::new(
            engine_handle(1, 1),
            engine_handle(2, 1),
            initial,
            RecordingDevice::default(),
        )
        .unwrap();

        assert_eq!(
            owner.rebind(initial, engine_handle(3, 1)),
            Err(AudioDeviceAdapterError::StalePermit)
        );
        assert_eq!(owner.device().activated, vec![initial]);
        assert!(owner.device().deactivated.is_empty());
    }

    #[test]
    fn valid_rebind_and_close_have_single_ordered_device_transitions() {
        let initial = permit(1);
        let next = permit(2);
        let mut owner = NativeAudioOwner::new(
            engine_handle(1, 1),
            engine_handle(2, 1),
            initial,
            RecordingDevice::default(),
        )
        .unwrap();

        owner.rebind(next, engine_handle(3, 1)).unwrap();
        owner.close().unwrap();
        owner.close().unwrap();

        assert_eq!(owner.device().activated, vec![initial, next]);
        assert_eq!(owner.device().deactivated, vec![initial, next]);
    }
}
