use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use vo_app_protocol::{
    AudioDeviceFormat, AudioDevicePermit, AudioRealtimeEndpoint, GenerationalHandle,
};
use voplay_audio::{CpalAudioDevice, NativeAudioOwner, RealtimePcmRing};
use voplay_protocol::Handle;

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

fn main() {
    let ring = Arc::new(RealtimePcmRing::new(48_000).expect("create realtime PCM ring"));
    ring.push(&vec![0; 24_000])
        .expect("preload silent realtime PCM");
    let device = CpalAudioDevice::new(Arc::clone(&ring));
    let mut owner =
        NativeAudioOwner::new(engine_handle(1, 1), engine_handle(2, 1), permit(1), device)
            .expect("activate default native audio output");

    let deadline = Instant::now() + Duration::from_secs(3);
    while owner.device().callback_invocation_count() == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let active_callbacks = owner.device().callback_invocation_count();
    assert!(
        active_callbacks > 0,
        "CoreAudio output callback did not run"
    );
    assert_eq!(owner.device().callback_error_count(), 0);

    owner
        .rebind(permit(2), engine_handle(3, 1))
        .expect("rebind native audio output generation");
    let callbacks_after_rebind = owner.device().callback_invocation_count();
    let recovery_deadline = Instant::now() + Duration::from_secs(3);
    while owner.device().callback_invocation_count() == callbacks_after_rebind
        && Instant::now() < recovery_deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    let recovered_callbacks = owner.device().callback_invocation_count();
    assert!(
        recovered_callbacks > callbacks_after_rebind,
        "CoreAudio callback did not resume after generation rebind"
    );
    assert_eq!(owner.device().callback_error_count(), 0);

    owner.close().expect("deactivate native audio output");
    thread::sleep(Duration::from_millis(100));
    let settled_callbacks = owner.device().callback_invocation_count();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        owner.device().callback_invocation_count(),
        settled_callbacks,
        "audio callback continued after explicit close"
    );

    println!(
        "{{\"passed\":true,\"active_callbacks\":{},\"recovered_callbacks\":{},\"settled_callbacks\":{},\"callback_errors\":{}}}",
        active_callbacks,
        recovered_callbacks,
        settled_callbacks,
        owner.device().callback_error_count()
    );
}
