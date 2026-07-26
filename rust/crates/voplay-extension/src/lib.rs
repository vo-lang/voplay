use std::collections::{BTreeMap, BTreeSet};

use vo_ext::prelude::{vo_fn, ExternCallContext, ExternResult, HostEventReplaySource};
use voplay_protocol::EngineId;
use voplay_runtime::{
    game_engine::{
        Game, GameEngine, GameEngineConfig, GameEngineError, GameEntryDescriptor, OwnedInitData,
        TargetIslandFactory,
    },
    profile::{
        validate_build_recipe, validate_role_artifacts, BuildRecipe, Capability, PlacementClass,
        ProfileError, RoleArtifact,
    },
    supervisor::Role,
};

#[cfg(any(feature = "profile-full", feature = "profile-editor"))]
mod full_encoder;

#[vo_fn("voplay", "TargetEngine")]
fn target_engine(call: &mut ExternCallContext) -> ExternResult {
    let Some(caller) = call.caller_endpoint_handle() else {
        return ExternResult::Panic(String::from(
            "Voplay target island has no valid App Runtime caller identity",
        ));
    };
    call.ret_u64(0, u64::from(caller.session_index));
    call.ret_u64(1, u64::from(caller.session_generation));
    call.ret_u64(2, caller.session_epoch);
    call.ret_u64(3, u64::from(caller.endpoint_index));
    call.ret_u64(4, u64::from(caller.endpoint_generation));
    ExternResult::Ok
}

#[vo_fn("voplay", "CurrentSession")]
fn current_session(call: &mut ExternCallContext) -> ExternResult {
    let Some(caller) = call.caller_endpoint_handle() else {
        for slot in 0..3 {
            call.ret_u64(slot, 0);
        }
        return ExternResult::Ok;
    };
    let Some(session_index) = caller.session_index.checked_add(1) else {
        for slot in 0..3 {
            call.ret_u64(slot, 0);
        }
        return ExternResult::Ok;
    };
    call.ret_u64(0, u64::from(session_index));
    call.ret_u64(1, u64::from(caller.session_generation));
    call.ret_u64(2, caller.session_epoch);
    ExternResult::Ok
}

#[vo_fn("voplay", "TargetStart")]
fn target_start(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(1) == 0 {
            call.ret_nil_error(0);
        } else {
            call.ret_error_msg(
                0,
                &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
            );
        }
        return ExternResult::Ok;
    }
    let configuration = call.arg_bytes(0).to_vec();
    if configuration.len() > 16 * 1024 * 1024
        || !configuration.starts_with(b"voplay-target-start-v3\0")
    {
        call.ret_error_msg(0, "generated Voplay target configuration is invalid");
        return ExternResult::Ok;
    }
    let token = call.next_host_event_token();
    match call.begin_host_request("voplay.target-start", &configuration, token, 0) {
        Ok(_) => ExternResult::HostEventWaitAndReplay {
            token,
            source: HostEventReplaySource::Extension,
        },
        Err(status) => {
            call.ret_error_msg(
                0,
                &format!("App Runtime rejected Voplay target startup with status {status}"),
            );
            ExternResult::Ok
        }
    }
}

#[vo_fn("voplay", "TargetNextTicks")]
fn target_next_ticks(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(1) == 0 {
            call.ret_bytes(0, response.get(1..).unwrap_or_default());
            call.ret_nil_error(1);
        } else {
            call.ret_bytes(0, &[]);
            call.ret_error_msg(
                1,
                &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
            );
        }
        return ExternResult::Ok;
    }
    let token = call.next_host_event_token();
    match call.begin_host_request("voplay.target-next-ticks", &[], token, 0) {
        Ok(_) => ExternResult::HostEventWaitAndReplay {
            token,
            source: HostEventReplaySource::Extension,
        },
        Err(status) => {
            call.ret_bytes(0, &[]);
            call.ret_error_msg(
                1,
                &format!("App Runtime rejected Voplay target tick wait with status {status}"),
            );
            ExternResult::Ok
        }
    }
}

#[vo_fn("voplay", "TargetCommitTicks")]
fn target_commit_ticks(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(1) == 0 {
            call.ret_nil_error(0);
        } else {
            call.ret_error_msg(
                0,
                &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
            );
        }
        return ExternResult::Ok;
    }
    let first_tick = call.arg_u64(0);
    let count = call.arg_u64(1);
    let result = call.arg_bytes(2);
    if first_tick == 0 || count == 0 || result.len() > 16 * 1024 * 1024 {
        call.ret_error_msg(0, "generated Voplay target tick commit is invalid");
        return ExternResult::Ok;
    }
    let mut payload = Vec::with_capacity(49 + result.len());
    payload.extend_from_slice(b"voplay-target-commit-ticks-v1\0");
    payload.extend_from_slice(&first_tick.to_le_bytes());
    payload.extend_from_slice(&count.to_le_bytes());
    payload.extend_from_slice(&(result.len() as u32).to_le_bytes());
    payload.extend_from_slice(result);
    let token = call.next_host_event_token();
    match call.begin_host_request("voplay.target-commit-ticks", &payload, token, 0) {
        Ok(_) => ExternResult::HostEventWaitAndReplay {
            token,
            source: HostEventReplaySource::Extension,
        },
        Err(status) => {
            call.ret_error_msg(
                0,
                &format!("App Runtime rejected Voplay target tick commit with status {status}"),
            );
            ExternResult::Ok
        }
    }
}

#[vo_fn("voplay", "RunEntry")]
fn run_entry(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(0) == 0 {
            call.ret_nil_error(0);
        } else {
            call.ret_error_msg(0, &String::from_utf8_lossy(&response[1..]));
        }
        return ExternResult::Ok;
    }
    let descriptor = call.arg_bytes(0).to_vec();
    let init = call.arg_bytes(1).to_vec();
    if descriptor.len() != 104 || init.len() > 16 * 1024 * 1024 {
        call.ret_error_msg(0, "generated entry launch payload is invalid");
        return ExternResult::Ok;
    }
    let mut payload = Vec::with_capacity(27 + descriptor.len() + init.len());
    payload.extend_from_slice(b"vo-entry-launch-v1\0");
    payload.extend_from_slice(&(descriptor.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(init.len() as u32).to_le_bytes());
    payload.extend_from_slice(&descriptor);
    payload.extend_from_slice(&init);
    let token = call.next_host_event_token();
    match call.begin_host_request("voplay.run-entry", &payload, token, 0) {
        Ok(_) => ExternResult::HostEventWaitAndReplay {
            token,
            source: HostEventReplaySource::Extension,
        },
        Err(status) => {
            call.ret_error_msg(
                0,
                &format!("App Runtime rejected generated entry request with status {status}"),
            );
            ExternResult::Ok
        }
    }
}

#[vo_fn("voplay", "InstallEntry")]
fn install_entry(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(1) == 0 {
            call.ret_nil_error(0);
        } else {
            call.ret_error_msg(
                0,
                &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
            );
        }
        return ExternResult::Ok;
    }
    let session_index = call.arg_u64(0);
    let session_generation = call.arg_u64(1);
    let session_epoch = call.arg_u64(2);
    let engine_index = call.arg_u64(3);
    let engine_generation = call.arg_u64(4);
    let descriptor = call.arg_bytes(5);
    let init = call.arg_bytes(6);
    if session_index >= u32::MAX.into()
        || session_generation == 0
        || session_generation > u32::MAX.into()
        || session_epoch == 0
        || engine_index >= u32::MAX.into()
        || engine_generation == 0
        || engine_generation > u32::MAX.into()
        || descriptor.len() != 104
        || init.len() > 16 * 1024 * 1024
    {
        call.ret_error_msg(0, "generated Voplay install payload is invalid");
        return ExternResult::Ok;
    }
    let mut payload = Vec::with_capacity(56 + descriptor.len() + init.len());
    payload.extend_from_slice(b"voplay-install-entry-v1\0");
    payload.extend_from_slice(&(session_index as u32).to_le_bytes());
    payload.extend_from_slice(&(session_generation as u32).to_le_bytes());
    payload.extend_from_slice(&session_epoch.to_le_bytes());
    payload.extend_from_slice(&(engine_index as u32).to_le_bytes());
    payload.extend_from_slice(&(engine_generation as u32).to_le_bytes());
    payload.extend_from_slice(&(descriptor.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(init.len() as u32).to_le_bytes());
    payload.extend_from_slice(descriptor);
    payload.extend_from_slice(init);
    begin_engine_request(call, "voplay.install-entry", &payload, 0)
}

#[vo_fn("voplay", "NewEngine")]
fn new_engine(call: &mut ExternCallContext) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.len() == 9 && response[0] == 0 {
            let index = u32::from_le_bytes(response[1..5].try_into().unwrap());
            let generation = u32::from_le_bytes(response[5..9].try_into().unwrap());
            if index != u32::MAX && generation != 0 {
                call.ret_u64(0, call.arg_u64(0));
                call.ret_u64(1, call.arg_u64(1));
                call.ret_u64(2, call.arg_u64(2));
                call.ret_u64(3, u64::from(index));
                call.ret_u64(4, u64::from(generation));
                call.ret_nil_error(5);
                return ExternResult::Ok;
            }
        }
        for slot in 0..5 {
            call.ret_u64(slot, 0);
        }
        call.ret_error_msg(
            5,
            &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
        );
        return ExternResult::Ok;
    }
    let session_index = call.arg_u64(0);
    let session_generation = call.arg_u64(1);
    let session_epoch = call.arg_u64(2);
    let profile = call.arg_string_bytes(3);
    let fixed_tick_nanos = call.arg_u64(4);
    let max_catch_up_ticks = call.arg_u64(5);
    let max_world_entities = call.arg_u64(6);
    let headless = call.arg_bool(7);
    if session_index >= u32::MAX.into()
        || session_generation == 0
        || session_generation > u32::MAX.into()
        || session_epoch == 0
        || profile.is_empty()
        || profile.len() > 64
        || fixed_tick_nanos == 0
        || max_catch_up_ticks == 0
        || max_catch_up_ticks > u32::MAX.into()
        || max_world_entities == 0
        || max_world_entities > u32::MAX.into()
    {
        for slot in 0..5 {
            call.ret_u64(slot, 0);
        }
        call.ret_error_msg(5, "Voplay EngineDesc is invalid");
        return ExternResult::Ok;
    }
    let mut payload = Vec::with_capacity(45 + profile.len());
    payload.extend_from_slice(b"voplay-new-engine-v1\0");
    payload.extend_from_slice(&(session_index as u32).to_le_bytes());
    payload.extend_from_slice(&(session_generation as u32).to_le_bytes());
    payload.extend_from_slice(&session_epoch.to_le_bytes());
    payload.extend_from_slice(&fixed_tick_nanos.to_le_bytes());
    payload.extend_from_slice(&(max_catch_up_ticks as u32).to_le_bytes());
    payload.extend_from_slice(&(max_world_entities as u32).to_le_bytes());
    payload.push(u8::from(headless));
    payload.extend_from_slice(&(profile.len() as u32).to_le_bytes());
    payload.extend_from_slice(&profile);
    begin_engine_request(call, "voplay.new-engine", &payload, 5)
}

#[vo_fn("voplay", "Start")]
fn start_engine(call: &mut ExternCallContext) -> ExternResult {
    engine_lifecycle_request(call, "voplay.engine-start", false)
}

#[vo_fn("voplay", "StepTicks")]
fn step_engine_ticks(call: &mut ExternCallContext) -> ExternResult {
    engine_lifecycle_request(call, "voplay.engine-step", true)
}

#[vo_fn("voplay", "Pause")]
fn pause_engine(call: &mut ExternCallContext) -> ExternResult {
    engine_lifecycle_request(call, "voplay.engine-pause", false)
}

#[vo_fn("voplay", "Resume")]
fn resume_engine(call: &mut ExternCallContext) -> ExternResult {
    engine_lifecycle_request(call, "voplay.engine-resume", false)
}

#[vo_fn("voplay", "Shutdown")]
fn shutdown_engine(call: &mut ExternCallContext) -> ExternResult {
    engine_lifecycle_request(call, "voplay.engine-shutdown", false)
}

fn engine_lifecycle_request(
    call: &mut ExternCallContext,
    capability: &'static str,
    has_count: bool,
) -> ExternResult {
    if call.take_resume_host_event_token().is_some() {
        let response = call.take_resume_host_event_data().unwrap_or_default();
        if response.first().copied().unwrap_or(1) == 0 {
            call.ret_nil_error(0);
        } else {
            call.ret_error_msg(
                0,
                &String::from_utf8_lossy(response.get(1..).unwrap_or_default()),
            );
        }
        return ExternResult::Ok;
    }
    let values = (0..5).map(|slot| call.arg_u64(slot)).collect::<Vec<_>>();
    let count = has_count.then(|| call.arg_u64(5)).unwrap_or(0);
    if values[0] >= u32::MAX.into()
        || values[1] == 0
        || values[1] > u32::MAX.into()
        || values[2] == 0
        || values[3] >= u32::MAX.into()
        || values[4] == 0
        || values[4] > u32::MAX.into()
        || (has_count && count == 0)
    {
        call.ret_error_msg(0, "Voplay EngineRef or lifecycle request is invalid");
        return ExternResult::Ok;
    }
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(b"voplay-engine-control-v1\0");
    payload.extend_from_slice(&(values[0] as u32).to_le_bytes());
    payload.extend_from_slice(&(values[1] as u32).to_le_bytes());
    payload.extend_from_slice(&values[2].to_le_bytes());
    payload.extend_from_slice(&(values[3] as u32).to_le_bytes());
    payload.extend_from_slice(&(values[4] as u32).to_le_bytes());
    if has_count {
        payload.extend_from_slice(&count.to_le_bytes());
    }
    begin_engine_request(call, capability, &payload, 0)
}

fn begin_engine_request(
    call: &mut ExternCallContext,
    capability: &'static str,
    payload: &[u8],
    error_slot: u16,
) -> ExternResult {
    let token = call.next_host_event_token();
    match call.begin_host_request(capability, payload, token, 0) {
        Ok(_) => ExternResult::HostEventWaitAndReplay {
            token,
            source: HostEventReplaySource::Extension,
        },
        Err(status) => {
            if error_slot == 5 {
                for slot in 0..5 {
                    call.ret_u64(slot, 0);
                }
            }
            call.ret_error_msg(
                error_slot,
                &format!("App Runtime rejected {capability} with status {status}"),
            );
            ExternResult::Ok
        }
    }
}

mod provider_export {
    use core::ffi::c_void;
    use std::collections::{BTreeMap, VecDeque};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
    use crate::full_encoder::FullFrameEncoder;
    use vo_app_runtime::provider_abi::{
        CallerEndpointHandle, HostByteSpan, HostResourceHandle, ProviderFactoryTableV2,
        ProviderInstanceAbiV2, VoHostServicesV2, HOST_SERVICE_STATUS_OK,
        HOST_SERVICE_STATUS_WOULD_BLOCK, MAX_PROVIDER_PACKET_BYTES, PROVIDER_FACTORY_ABI_VERSION,
        PROVIDER_STATUS_INTERNAL_ERROR, PROVIDER_STATUS_INVALID_ARGUMENT,
        PROVIDER_STATUS_INVALID_STATE, PROVIDER_STATUS_OK,
    };
    use voplay_protocol::{decode_packet, encode_packet, EngineId, Handle};
    #[cfg(all(
        feature = "profile-2d",
        not(any(
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))
    ))]
    use voplay_render_2d::{
        ReferenceDrawResolver2d, Scene2dFrameEncoder, Scene2dFrameEncoderConfig,
    };
    #[cfg(all(
        feature = "profile-3d",
        not(any(feature = "profile-full", feature = "profile-editor"))
    ))]
    use voplay_render_3d::{Scene3dFrameEncoder, Scene3dFrameEncoderConfig};
    use voplay_runtime::asset_endpoint::{AssetEndpointDriver, AssetEndpointDriverConfig};
    #[cfg(all(
        any(feature = "profile-full", feature = "profile-editor"),
        any(not(feature = "native-audio"), target_arch = "wasm32")
    ))]
    use voplay_runtime::audio_endpoint::MixerAudioEventExecutor;
    #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
    use voplay_runtime::audio_endpoint::{
        AudioEndpointDriver, AudioEndpointDriverConfig, AudioEventExecutor,
    };
    use voplay_runtime::endpoint::{EndpointDriver, OwnedEndpointPacket};
    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    use voplay_runtime::host_render_backend::HostRenderBackend;
    use voplay_runtime::hosted_logic_endpoint::HostedLogicEndpointDriver;
    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    use voplay_runtime::render_control::RenderControlDecodeConfig;
    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    use voplay_runtime::render_endpoint::{
        DefaultRenderEventExecutor, RenderEndpointDriver, RenderEndpointPresentationConfig,
    };
    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    use voplay_runtime::surface::{RenderBackend, RenderBackendFailure};
    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    use voplay_runtime::EngineConfig;

    include!(concat!(env!("OUT_DIR"), "/provider_profile.rs"));

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Phase {
        Created,
        Prepared,
        Running,
        Suspended,
        Closed,
    }

    struct ProviderState {
        role: u32,
        caller: CallerEndpointHandle,
        host: VoHostServicesV2,
        phase: Phase,
        drivers: BTreeMap<EngineId, ProviderDriver>,
        pending_output: VecDeque<Vec<u8>>,
        pending_output_bytes: usize,
        #[cfg(any(
            feature = "profile-2d",
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        legacy_render_features: Option<Vec<Vec<u8>>>,
        #[cfg(any(
            feature = "profile-2d",
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        render_features: BTreeMap<EngineId, Vec<Vec<u8>>>,
        #[cfg(any(
            feature = "profile-2d",
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        render_result_sinks:
            BTreeMap<EngineId, voplay_runtime::host_render_backend::HostRenderResultSink>,
    }

    enum ProviderDriver {
        Logic(HostedLogicEndpointDriver),
        Asset(AssetEndpointDriver),
        #[cfg(any(
            feature = "profile-2d",
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        Render(RenderEndpointDriver),
        #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
        Audio(AudioEndpointDriver),
    }

    static PROVIDER_TABLE: ProviderFactoryTableV2 = ProviderFactoryTableV2 {
        struct_size: core::mem::size_of::<ProviderFactoryTableV2>() as u32,
        abi_version: PROVIDER_FACTORY_ABI_VERSION,
        factory_count: PROVIDER_FACTORIES.len() as u32,
        factories: PROVIDER_FACTORIES.as_ptr(),
    };

    #[no_mangle]
    pub extern "C" fn vo_provider_factories_v2() -> *const ProviderFactoryTableV2 {
        &PROVIDER_TABLE
    }

    #[allow(dead_code)]
    unsafe extern "C" fn create_game_logic_provider(
        host: *const VoHostServicesV2,
        caller: CallerEndpointHandle,
        out: *mut ProviderInstanceAbiV2,
    ) -> u32 {
        create_provider(
            vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_LOGIC,
            host,
            caller,
            out,
        )
    }

    #[allow(dead_code)]
    unsafe extern "C" fn create_game_asset_provider(
        host: *const VoHostServicesV2,
        caller: CallerEndpointHandle,
        out: *mut ProviderInstanceAbiV2,
    ) -> u32 {
        create_provider(
            vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_ASSET,
            host,
            caller,
            out,
        )
    }

    #[allow(dead_code)]
    unsafe extern "C" fn create_game_render_provider(
        host: *const VoHostServicesV2,
        caller: CallerEndpointHandle,
        out: *mut ProviderInstanceAbiV2,
    ) -> u32 {
        create_provider(
            vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER,
            host,
            caller,
            out,
        )
    }

    #[allow(dead_code)]
    unsafe extern "C" fn create_game_audio_provider(
        host: *const VoHostServicesV2,
        caller: CallerEndpointHandle,
        out: *mut ProviderInstanceAbiV2,
    ) -> u32 {
        create_provider(
            vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_AUDIO,
            host,
            caller,
            out,
        )
    }

    fn create_provider(
        role: u32,
        host: *const VoHostServicesV2,
        caller: CallerEndpointHandle,
        out: *mut ProviderInstanceAbiV2,
    ) -> u32 {
        if host.is_null() || out.is_null() || !caller.is_valid() {
            return PROVIDER_STATUS_INVALID_ARGUMENT;
        }
        let host = unsafe { *host };
        if host.validate().is_err() {
            return PROVIDER_STATUS_INVALID_ARGUMENT;
        }
        match catch_unwind(AssertUnwindSafe(|| {
            force_link_profile_closure();
            let state = Box::new(ProviderState {
                role,
                caller,
                host,
                phase: Phase::Created,
                drivers: BTreeMap::new(),
                pending_output: VecDeque::new(),
                pending_output_bytes: 0,
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                legacy_render_features: None,
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                render_features: BTreeMap::new(),
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                render_result_sinks: BTreeMap::new(),
            });
            let context = Box::into_raw(state).cast::<c_void>();
            unsafe {
                out.write(ProviderInstanceAbiV2 {
                    struct_size: core::mem::size_of::<ProviderInstanceAbiV2>() as u32,
                    context,
                    prepare: Some(prepare),
                    start: Some(start),
                    suspend: Some(suspend),
                    resume: Some(resume),
                    dispatch_packet: Some(dispatch_packet),
                    close: Some(close),
                    destroy: Some(destroy),
                });
            }
        })) {
            Ok(()) => PROVIDER_STATUS_OK,
            Err(_) => PROVIDER_STATUS_INTERNAL_ERROR,
        }
    }

    unsafe extern "C" fn prepare(context: *mut c_void) -> u32 {
        transition(context, Phase::Created, Phase::Prepared)
    }

    unsafe extern "C" fn start(context: *mut c_void) -> u32 {
        transition(context, Phase::Prepared, Phase::Running)
    }

    unsafe extern "C" fn suspend(context: *mut c_void) -> u32 {
        transition(context, Phase::Running, Phase::Suspended)
    }

    unsafe extern "C" fn resume(context: *mut c_void) -> u32 {
        transition(context, Phase::Suspended, Phase::Running)
    }

    unsafe extern "C" fn dispatch_packet(context: *mut c_void, packet: HostByteSpan) -> u32 {
        abi_guard(|| {
            let state = unsafe { context.cast::<ProviderState>().as_mut() }
                .ok_or(PROVIDER_STATUS_INVALID_ARGUMENT)?;
            if state.phase != Phase::Running
                || packet.reserved != 0
                || packet.ptr.is_null()
                || packet.len == 0
                || packet.len as usize > MAX_PROVIDER_PACKET_BYTES
            {
                return Err(PROVIDER_STATUS_INVALID_ARGUMENT);
            }
            state.flush_pending_output()?;
            if !state.pending_output.is_empty() {
                return Err(PROVIDER_STATUS_INVALID_STATE);
            }
            let packet = unsafe { core::slice::from_raw_parts(packet.ptr, packet.len as usize) };
            state.dispatch(packet)?;
            state.flush_pending_output()
        })
    }

    unsafe extern "C" fn close(context: *mut c_void) -> u32 {
        abi_guard(|| {
            let state = unsafe { context.cast::<ProviderState>().as_mut() }
                .ok_or(PROVIDER_STATUS_INVALID_ARGUMENT)?;
            for driver in state.drivers.values_mut() {
                driver
                    .close(0)
                    .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)?;
            }
            state.collect_all_output()?;
            state.flush_pending_output()?;
            state.drivers.clear();
            #[cfg(any(
                feature = "profile-2d",
                feature = "profile-3d",
                feature = "profile-full",
                feature = "profile-editor"
            ))]
            state.render_result_sinks.clear();
            state.phase = Phase::Closed;
            Ok(())
        })
    }

    unsafe extern "C" fn destroy(context: *mut c_void) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if !context.is_null() {
                drop(unsafe { Box::from_raw(context.cast::<ProviderState>()) });
            }
        }));
    }

    fn transition(context: *mut c_void, from: Phase, to: Phase) -> u32 {
        abi_guard(|| {
            let state = unsafe { context.cast::<ProviderState>().as_mut() }
                .ok_or(PROVIDER_STATUS_INVALID_ARGUMENT)?;
            if state.phase != from {
                return Err(PROVIDER_STATUS_INVALID_STATE);
            }
            state.phase = to;
            Ok(())
        })
    }

    impl ProviderState {
        fn dispatch(&mut self, packet: &[u8]) -> Result<(), u32> {
            #[cfg(any(
                feature = "profile-2d",
                feature = "profile-3d",
                feature = "profile-full",
                feature = "profile-editor"
            ))]
            if packet.starts_with(b"VHR4") {
                if self.role != vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                let (engine, result) =
                    voplay_runtime::host_render_backend::decode_routed_host_render_result(packet)
                        .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?;
                return self
                    .render_result_sinks
                    .get(&engine)
                    .ok_or(PROVIDER_STATUS_INVALID_STATE)?
                    .submit(result)
                    .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT);
            }
            #[cfg(any(
                feature = "profile-2d",
                feature = "profile-3d",
                feature = "profile-full",
                feature = "profile-editor"
            ))]
            if packet.starts_with(b"VHR2") {
                if self.role != vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                let matching = self
                    .render_result_sinks
                    .iter()
                    .filter_map(|(engine, sink)| match sink.accepts(packet) {
                        Ok(true) => Some(Ok(*engine)),
                        Ok(false) => None,
                        Err(_) => Some(Err(PROVIDER_STATUS_INVALID_ARGUMENT)),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if matching.len() != 1 {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                let engine = matching[0];
                return self
                    .render_result_sinks
                    .get(&engine)
                    .ok_or(PROVIDER_STATUS_INVALID_STATE)?
                    .submit(packet)
                    .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT);
            }
            #[cfg(any(
                feature = "profile-2d",
                feature = "profile-3d",
                feature = "profile-full",
                feature = "profile-editor"
            ))]
            if packet.starts_with(b"VFRB2\0\0\0") {
                if self.role != vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                let (engine, features) =
                    voplay_runtime::render_feature::decode_routed_feature_bootstrap(packet)
                        .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?;
                if self.drivers.contains_key(&engine) || self.render_features.contains_key(&engine)
                {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                self.render_features.insert(
                    engine,
                    features.into_iter().map(<[u8]>::to_vec).collect::<Vec<_>>(),
                );
                return Ok(());
            }
            #[cfg(any(
                feature = "profile-2d",
                feature = "profile-3d",
                feature = "profile-full",
                feature = "profile-editor"
            ))]
            if packet.starts_with(b"VFRB1\0\0\0") {
                if self.role != vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER
                    || !self.drivers.is_empty()
                    || self.legacy_render_features.is_some()
                    || !self.render_features.is_empty()
                {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                self.legacy_render_features = Some(
                    voplay_runtime::render_feature::decode_feature_bootstrap(packet)
                        .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?
                        .into_iter()
                        .map(<[u8]>::to_vec)
                        .collect(),
                );
                return Ok(());
            }
            let (header, payload) =
                decode_packet(packet).map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?;
            if !self.drivers.contains_key(&header.engine) {
                if self.drivers.len() >= 1024 {
                    return Err(PROVIDER_STATUS_INVALID_STATE);
                }
                #[allow(unused_mut)]
                let mut driver = self.new_driver(header.engine)?;
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                if let ProviderDriver::Render(render) = &mut driver {
                    let features = self
                        .render_features
                        .get(&header.engine)
                        .or(self.legacy_render_features.as_ref())
                        .ok_or(PROVIDER_STATUS_INVALID_STATE)?;
                    for feature in features {
                        render
                            .register_render_feature(feature)
                            .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?;
                    }
                }
                self.drivers.insert(header.engine, driver);
            }
            self.drivers
                .get_mut(&header.engine)
                .ok_or(PROVIDER_STATUS_INTERNAL_ERROR)?
                .dispatch(header, payload)
                .map(|_| ())
                .map_err(|_| PROVIDER_STATUS_INVALID_ARGUMENT)?;
            self.collect_output(header.engine)
        }

        fn new_driver(&mut self, engine: EngineId) -> Result<ProviderDriver, u32> {
            let endpoint_generation = Handle {
                index: self.caller.endpoint_index,
                generation: self.caller.endpoint_generation,
            };
            match self.role {
                vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_ASSET => AssetEndpointDriver::new(
                    engine,
                    endpoint_generation,
                    AssetEndpointDriverConfig::default(),
                )
                .map(ProviderDriver::Asset)
                .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR),
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER => {
                    let mut presentation_config = RenderEndpointPresentationConfig::default();
                    presentation_config.endpoint_generation = endpoint_generation;
                    let (backend, sink) = host_render_backend(self.host, self.caller, engine)?;
                    let driver = RenderEndpointDriver::new_with_presentation(
                        engine,
                        EngineConfig::default(),
                        RenderControlDecodeConfig::default(),
                        Box::new(DefaultRenderEventExecutor),
                        presentation_config,
                        backend,
                        profile_frame_encoder(engine)?,
                    )
                    .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)?;
                    self.render_result_sinks.insert(engine, sink);
                    Ok(ProviderDriver::Render(driver))
                }
                #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
                vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_AUDIO => {
                    let executor = audio_executor(engine, endpoint_generation)?;
                    AudioEndpointDriver::new(
                        engine,
                        endpoint_generation,
                        AudioEndpointDriverConfig::default(),
                        executor,
                    )
                    .map(ProviderDriver::Audio)
                    .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
                }
                vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_LOGIC => {
                    HostedLogicEndpointDriver::new(engine)
                        .map(ProviderDriver::Logic)
                        .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
                }
                _ => Err(PROVIDER_STATUS_INVALID_ARGUMENT),
            }
        }

        fn collect_output(&mut self, engine: EngineId) -> Result<(), u32> {
            let packets = self
                .drivers
                .get_mut(&engine)
                .ok_or(PROVIDER_STATUS_INTERNAL_ERROR)?
                .poll(4096)
                .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)?;
            self.enqueue_output_packets(packets)
        }

        fn collect_all_output(&mut self) -> Result<(), u32> {
            let engines = self.drivers.keys().copied().collect::<Vec<_>>();
            for engine in engines {
                self.collect_output(engine)?;
            }
            Ok(())
        }

        fn enqueue_output_packets(&mut self, packets: Vec<OwnedEndpointPacket>) -> Result<(), u32> {
            let mut encoded = Vec::with_capacity(packets.len());
            let mut added_bytes = 0_usize;
            for packet in packets {
                let bytes = encode_owned_packet(packet)?;
                added_bytes = added_bytes
                    .checked_add(bytes.len())
                    .ok_or(PROVIDER_STATUS_INTERNAL_ERROR)?;
                encoded.push(bytes);
            }
            if self.pending_output.len().saturating_add(encoded.len()) > 4096
                || self
                    .pending_output_bytes
                    .checked_add(added_bytes)
                    .is_none_or(|bytes| bytes > 16 * 1024 * 1024)
            {
                return Err(PROVIDER_STATUS_INVALID_STATE);
            }
            for packet in encoded {
                self.pending_output_bytes += packet.len();
                self.pending_output.push_back(packet);
            }
            Ok(())
        }

        fn flush_pending_output(&mut self) -> Result<(), u32> {
            let publish = self
                .host
                .publish_endpoint_packet
                .ok_or(PROVIDER_STATUS_INTERNAL_ERROR)?;
            while let Some(packet) = self.pending_output.front() {
                let status = unsafe {
                    publish(
                        self.host.context,
                        self.caller,
                        HostResourceHandle::INVALID,
                        0,
                        HostByteSpan {
                            ptr: packet.as_ptr(),
                            len: packet.len() as u32,
                            reserved: 0,
                        },
                    )
                };
                if status == HOST_SERVICE_STATUS_WOULD_BLOCK {
                    break;
                }
                if status != HOST_SERVICE_STATUS_OK {
                    return Err(PROVIDER_STATUS_INTERNAL_ERROR);
                }
                let packet = self.pending_output.pop_front().unwrap();
                self.pending_output_bytes -= packet.len();
            }
            Ok(())
        }
    }

    #[cfg(any(
        feature = "profile-2d",
        feature = "profile-3d",
        feature = "profile-full",
        feature = "profile-editor"
    ))]
    fn host_render_backend(
        host: VoHostServicesV2,
        caller: CallerEndpointHandle,
        engine: EngineId,
    ) -> Result<
        (
            Box<dyn RenderBackend>,
            voplay_runtime::host_render_backend::HostRenderResultSink,
        ),
        u32,
    > {
        let publish = host
            .publish_endpoint_packet
            .ok_or(PROVIDER_STATUS_INTERNAL_ERROR)?;
        HostRenderBackend::new_with_result_sink(
            16,
            Box::new(move |packet| {
                let packet =
                    voplay_runtime::host_render_backend::encode_routed_host_render_command(
                        engine, packet,
                    )
                    .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
                let status = unsafe {
                    publish(
                        host.context,
                        caller,
                        HostResourceHandle::INVALID,
                        0,
                        HostByteSpan {
                            ptr: packet.as_ptr(),
                            len: packet.len() as u32,
                            reserved: 0,
                        },
                    )
                };
                match status {
                    HOST_SERVICE_STATUS_OK => Ok(()),
                    HOST_SERVICE_STATUS_WOULD_BLOCK => {
                        Err(RenderBackendFailure::RejectedBeforeSubmit)
                    }
                    _ => Err(RenderBackendFailure::DeviceLost),
                }
            }),
        )
        .map(|(backend, sink)| (Box::new(backend) as Box<dyn RenderBackend>, sink))
        .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
    }

    #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
    fn audio_executor(
        engine: EngineId,
        endpoint_generation: Handle,
    ) -> Result<Box<dyn AudioEventExecutor>, u32> {
        #[cfg(all(feature = "native-audio", not(target_arch = "wasm32")))]
        {
            return voplay_audio::CpalAudioEventExecutor::new(
                engine,
                endpoint_generation,
                48_000 * 2,
            )
            .map(|executor| Box::new(executor) as Box<dyn AudioEventExecutor>)
            .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR);
        }
        #[cfg(any(not(feature = "native-audio"), target_arch = "wasm32"))]
        {
            let _ = (engine, endpoint_generation);
            Ok(Box::new(MixerAudioEventExecutor))
        }
    }

    #[cfg(all(
        feature = "profile-2d",
        not(any(
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))
    ))]
    fn profile_frame_encoder(
        _engine: EngineId,
    ) -> Result<Box<dyn voplay_runtime::presentation::FrameCommandEncoder>, u32> {
        Scene2dFrameEncoder::new(
            Scene2dFrameEncoderConfig::default(),
            ReferenceDrawResolver2d,
        )
        .map(|encoder| {
            Box::new(encoder) as Box<dyn voplay_runtime::presentation::FrameCommandEncoder>
        })
        .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
    }

    #[cfg(all(
        feature = "profile-3d",
        not(any(feature = "profile-full", feature = "profile-editor"))
    ))]
    fn profile_frame_encoder(
        engine: EngineId,
    ) -> Result<Box<dyn voplay_runtime::presentation::FrameCommandEncoder>, u32> {
        Scene3dFrameEncoder::new(engine, Scene3dFrameEncoderConfig::default())
            .and_then(|mut encoder| {
                for factory in PROFILE_RENDER_FEATURE_FACTORIES {
                    encoder.register_render_feature_factory(factory)?;
                }
                Ok(encoder)
            })
            .map(|encoder| {
                Box::new(encoder) as Box<dyn voplay_runtime::presentation::FrameCommandEncoder>
            })
            .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
    }

    #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
    fn profile_frame_encoder(
        engine: EngineId,
    ) -> Result<Box<dyn voplay_runtime::presentation::FrameCommandEncoder>, u32> {
        FullFrameEncoder::new_with_render_feature_factories(
            engine,
            &PROFILE_RENDER_FEATURE_FACTORIES,
        )
        .map(|encoder| {
            Box::new(encoder) as Box<dyn voplay_runtime::presentation::FrameCommandEncoder>
        })
        .map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
    }

    impl ProviderDriver {
        fn dispatch(
            &mut self,
            header: voplay_protocol::PacketHeader,
            payload: &[u8],
        ) -> Result<
            voplay_runtime::endpoint::DispatchOutcome,
            voplay_runtime::endpoint::EndpointDriverError,
        > {
            match self {
                Self::Logic(driver) => driver.dispatch(header, payload),
                Self::Asset(driver) => driver.dispatch(header, payload),
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                Self::Render(driver) => driver.dispatch(header, payload),
                #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
                Self::Audio(driver) => driver.dispatch(header, payload),
            }
        }

        fn poll(
            &mut self,
            budget: usize,
        ) -> Result<Vec<OwnedEndpointPacket>, voplay_runtime::endpoint::EndpointDriverError>
        {
            match self {
                Self::Logic(driver) => driver.poll(budget),
                Self::Asset(driver) => driver.poll(budget),
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                Self::Render(driver) => driver.poll(budget),
                #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
                Self::Audio(driver) => driver.poll(budget),
            }
        }

        fn close(
            &mut self,
            deadline_micros: u64,
        ) -> Result<(), voplay_runtime::endpoint::EndpointDriverError> {
            match self {
                Self::Logic(driver) => driver.close(deadline_micros),
                Self::Asset(driver) => driver.close(deadline_micros),
                #[cfg(any(
                    feature = "profile-2d",
                    feature = "profile-3d",
                    feature = "profile-full",
                    feature = "profile-editor"
                ))]
                Self::Render(driver) => driver.close(deadline_micros),
                #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
                Self::Audio(driver) => driver.close(deadline_micros),
            }
        }
    }

    fn encode_owned_packet(packet: OwnedEndpointPacket) -> Result<Vec<u8>, u32> {
        encode_packet(packet.header, &packet.payload).map_err(|_| PROVIDER_STATUS_INTERNAL_ERROR)
    }

    fn abi_guard(operation: impl FnOnce() -> Result<(), u32>) -> u32 {
        match catch_unwind(AssertUnwindSafe(operation)) {
            Ok(Ok(())) => PROVIDER_STATUS_OK,
            Ok(Err(status)) => status,
            Err(_) => PROVIDER_STATUS_INTERNAL_ERROR,
        }
    }

    fn force_link_profile_closure() {
        #[allow(unused_mut)]
        let mut identity = voplay_assets::voplay_profile_link_anchor();
        #[cfg(any(
            feature = "profile-2d",
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        {
            identity ^= voplay_render_core::voplay_profile_link_anchor();
            identity ^= voplay_render_2d::voplay_profile_link_anchor();
        }
        #[cfg(feature = "profile-2d")]
        {
            identity ^= voplay_physics_2d::voplay_profile_link_anchor();
        }
        #[cfg(any(
            feature = "profile-3d",
            feature = "profile-full",
            feature = "profile-editor"
        ))]
        {
            identity ^= voplay_animation::voplay_profile_link_anchor();
            identity ^= voplay_import_gltf::voplay_profile_link_anchor();
            identity ^= voplay_physics_3d::voplay_profile_link_anchor();
            identity ^= voplay_render_3d::voplay_profile_link_anchor();
        }
        #[cfg(any(feature = "profile-full", feature = "profile-editor"))]
        {
            identity ^= voplay_audio::voplay_profile_link_anchor();
            identity ^= voplay_physics_2d::voplay_profile_link_anchor();
        }
        #[cfg(feature = "profile-editor")]
        {
            identity ^= voplay_vogui_editor::voplay_profile_link_anchor();
        }
        std::hint::black_box(identity);
    }
}

pub const fn compiled_profile_name() -> &'static str {
    provider_export::PROFILE_NAME
}

vo_ext::export_extensions!(vo_ext::vo_extension_entry!("voplay", "RunEntry"));
vo_ext::export_wasm_extension_protocol!();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedRoleArtifact {
    pub role: Role,
    pub placement: PlacementClass,
    pub artifact_name: String,
    pub target: String,
    pub digest: [u8; 32],
    pub schema_fingerprint: [u8; 32],
    pub shader_abi: Option<[u8; 32]>,
    pub realtime_abi: Option<[u8; 32]>,
    pub capabilities: BTreeSet<Capability>,
}

pub fn materialize(
    recipe: BuildRecipe,
    target: &str,
    published: Vec<PublishedRoleArtifact>,
) -> Result<(BuildRecipe, BTreeMap<Role, RoleArtifact>), ProfileError> {
    validate_build_recipe(&recipe)?;
    let artifacts = published
        .into_iter()
        .map(|artifact| RoleArtifact {
            role: artifact.role,
            placement: artifact.placement,
            artifact_name: artifact.artifact_name,
            target: artifact.target,
            digest: artifact.digest,
            schema_fingerprint: artifact.schema_fingerprint,
            shader_abi: artifact.shader_abi,
            realtime_abi: artifact.realtime_abi,
            capabilities: artifact.capabilities,
        })
        .collect::<Vec<_>>();
    if artifacts.iter().any(|artifact| artifact.target != target) {
        let role = artifacts
            .iter()
            .find(|artifact| artifact.target != target)
            .map(|artifact| artifact.role)
            .unwrap_or(Role::Logic);
        return Err(ProfileError::InvalidArtifact(role));
    }
    let artifacts = validate_role_artifacts(&recipe.profile, artifacts)?;
    Ok((recipe, artifacts))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    Profile(ProfileError),
    DescriptorRoles,
    Game(GameEngineError),
}

impl From<ProfileError> for LaunchError {
    fn from(error: ProfileError) -> Self {
        Self::Profile(error)
    }
}

impl From<GameEngineError> for LaunchError {
    fn from(error: GameEngineError) -> Self {
        Self::Game(error)
    }
}

pub struct ProfiledEngine<G: Game> {
    recipe: BuildRecipe,
    artifacts: BTreeMap<Role, RoleArtifact>,
    engine: GameEngine<G>,
    closed: bool,
}

impl<G: Game> ProfiledEngine<G> {
    pub const fn recipe(&self) -> &BuildRecipe {
        &self.recipe
    }

    pub const fn artifacts(&self) -> &BTreeMap<Role, RoleArtifact> {
        &self.artifacts
    }

    pub const fn engine(&self) -> &GameEngine<G> {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut GameEngine<G> {
        &mut self.engine
    }

    pub fn shutdown(mut self) -> Result<(), LaunchError> {
        self.engine.shutdown()?;
        self.closed = true;
        Ok(())
    }
}

impl<G: Game> Drop for ProfiledEngine<G> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.engine.abandon();
            self.closed = true;
        }
    }
}

pub fn launch<F>(
    id: EngineId,
    recipe: BuildRecipe,
    target: &str,
    published: Vec<PublishedRoleArtifact>,
    descriptor: GameEntryDescriptor,
    factory: &mut F,
    init: OwnedInitData,
    config: GameEngineConfig,
) -> Result<ProfiledEngine<F::Game>, LaunchError>
where
    F: TargetIslandFactory,
{
    let (recipe, artifacts) = materialize(recipe, target, published)?;
    if descriptor.roles != recipe.profile.required_roles
        || config.group.roles != recipe.profile.required_roles
    {
        return Err(LaunchError::DescriptorRoles);
    }
    let mut engine = GameEngine::install(id, descriptor, factory, init, config)?;
    engine.start()?;
    Ok(ProfiledEngine {
        recipe,
        artifacts,
        engine,
        closed: false,
    })
}
