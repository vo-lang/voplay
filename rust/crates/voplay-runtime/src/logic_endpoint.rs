use std::collections::{BTreeSet, VecDeque};

use vo_app_protocol::SurfaceHandle;
use voplay_protocol::{decode_packet, generated::MessageKind, Handle, PacketHeader};

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
    asset::{AssetRef, AssetScopeId},
    audio::AudioEvent,
    control::{ControlDomain, ControlStateSnapshot},
    control_wire::{encode_audio_control_snapshot, encode_render_control_snapshot},
    endpoint::{
        DispatchOutcome, EndpointDriver, EndpointDriverError, EndpointQueueMetrics,
        OwnedEndpointPacket,
    },
    game_engine::{Game, GameEngine, GameEngineError, GameEngineState},
    haptics_wire::{decode_haptics_result_parts, encode_haptics_command},
    logic_io::LogicIoCommand,
    outbox::PresentationDomainId,
    readback_wire::{
        decode_readback_result, encode_readback_cancel, encode_readback_request,
        MAX_WIRE_READBACK_BYTES,
    },
    render_asset_outbox::{RenderAssetKey, RenderAssetOutbox, RenderAssetOutboxConfig},
    render_readback::{ReadbackRequest, ReadbackRequestId, ReadbackResult},
    render_wire::{decode_render_ack_parts, encode_render_transaction, encode_render_transient},
    simulation_state::SimulationEvent,
    surface::{GameSurfaceId, PresentationPulse, SurfaceMetrics},
    world::InputFrame,
    RenderEvent,
};
#[cfg(feature = "inspection")]
use crate::{
    inspection::{InspectionConfig, InspectionSchemaRegistry, InspectionService},
    inspection_endpoint::{
        InspectionEndpointReply, InspectionEndpointRuntime, InspectionRequestControl,
    },
};

pub const ASSET_COMPLETION_EVENT_KIND: u32 = 0x5650_0101;
pub const AUDIO_RESULT_EVENT_KIND: u32 = 0x5650_0102;
pub const RENDER_RESULT_EVENT_KIND: u32 = 0x5650_0103;
pub const HAPTICS_RESULT_EVENT_KIND: u32 = 0x5650_0105;
pub const RENDER_ASSET_ACK_EVENT_KIND: u32 = 0x5650_0106;
pub const AUDIO_ASSET_ACK_EVENT_KIND: u32 = 0x5650_0107;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicEndpointConfig {
    pub max_output_packets: usize,
    pub max_output_bytes: usize,
    pub asset_channel_epoch: u64,
    pub audio_channel_epoch: u64,
    pub render_channel_epoch: u64,
    pub render_assets: RenderAssetOutboxConfig,
    pub audio_assets: RenderAssetOutboxConfig,
    pub max_readback_results: usize,
    pub max_readback_bytes: usize,
}

impl Default for LogicEndpointConfig {
    fn default() -> Self {
        Self {
            max_output_packets: 64,
            max_output_bytes: 128 * 1024 * 1024,
            asset_channel_epoch: 1,
            audio_channel_epoch: 1,
            render_channel_epoch: 1,
            render_assets: RenderAssetOutboxConfig::default(),
            audio_assets: RenderAssetOutboxConfig {
                max_assets: 4096,
                max_bytes: 512 * 1024 * 1024,
                max_inflight: 128,
            },
            max_readback_results: 256,
            max_readback_bytes: MAX_WIRE_READBACK_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicEndpointState {
    Created,
    Starting,
    Running,
    Suspended,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicEndpointDriverOwnerSnapshot {
    pub state: LogicEndpointState,
    pub game: crate::game_engine::GameEngineOwnerSnapshot,
    pub startup_asset_controls: usize,
    pub render_assets: crate::render_asset_outbox::RenderAssetOutboxOwnerSnapshot,
    pub audio_assets: crate::render_asset_outbox::RenderAssetOutboxOwnerSnapshot,
    pub readback_results: usize,
    pub readback_result_bytes: usize,
    #[cfg(feature = "inspection")]
    pub inspection: crate::inspection_endpoint::InspectionEndpointOwnerSnapshot,
    pub output: EndpointQueueMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicEndpointDriverShutdownReport {
    pub game_before: crate::game_engine::GameEngineOwnerSnapshot,
    pub game_after: crate::game_engine::GameEngineOwnerSnapshot,
    pub render_assets: crate::render_asset_outbox::RenderAssetOutboxShutdownReport,
    pub audio_assets: crate::render_asset_outbox::RenderAssetOutboxShutdownReport,
    pub released_startup_asset_controls: usize,
    pub released_readback_results: usize,
    pub released_readback_result_bytes: usize,
    #[cfg(feature = "inspection")]
    pub inspection: crate::inspection_endpoint::InspectionEndpointShutdownReport,
}

pub struct LogicEndpointDriver<G: Game> {
    game: GameEngine<G>,
    config: LogicEndpointConfig,
    state: LogicEndpointState,
    output: VecDeque<OwnedEndpointPacket>,
    output_bytes: usize,
    output_peak_packets: usize,
    output_peak_bytes: usize,
    output_capacity_rejections: u64,
    pending_startup_asset_controls: BTreeSet<u64>,
    startup_render_control_ack: u64,
    startup_audio_control_ack: u64,
    next_haptics_sequence: u64,
    render_assets: RenderAssetOutbox,
    audio_assets: RenderAssetOutbox,
    readback_results: VecDeque<ReadbackResult>,
    readback_result_bytes: usize,
    #[cfg(feature = "inspection")]
    inspection: InspectionEndpointRuntime,
    #[cfg(feature = "inspection")]
    faults: FaultInjector,
}

impl<G: Game> LogicEndpointDriver<G> {
    pub fn new(game: GameEngine<G>, config: LogicEndpointConfig) -> Result<Self, GameEngineError> {
        #[cfg(feature = "inspection")]
        {
            let engine = game.id();
            let mut schemas = InspectionSchemaRegistry::new(engine, InspectionConfig::default())
                .map_err(|_| GameEngineError::InvalidConfig)?;
            schemas
                .freeze()
                .map_err(|_| GameEngineError::InvalidConfig)?;
            let inspection = InspectionEndpointRuntime::new(
                engine,
                schemas,
                InspectionService::new(engine, InspectionConfig::default())
                    .map_err(|_| GameEngineError::InvalidConfig)?,
            )
            .map_err(|_| GameEngineError::InvalidConfig)?;
            return Self::new_with_inspection(game, config, inspection);
        }
        #[cfg(not(feature = "inspection"))]
        {
            Self::new_inner(game, config)
        }
    }

    #[cfg(feature = "inspection")]
    pub fn new_with_inspection(
        game: GameEngine<G>,
        config: LogicEndpointConfig,
        inspection: InspectionEndpointRuntime,
    ) -> Result<Self, GameEngineError> {
        if inspection.service().engine() != game.id() {
            return Err(GameEngineError::InvalidConfig);
        }
        Self::new_inner(game, config, inspection)
    }

    #[cfg(feature = "inspection")]
    fn new_inner(
        game: GameEngine<G>,
        config: LogicEndpointConfig,
        inspection: InspectionEndpointRuntime,
    ) -> Result<Self, GameEngineError> {
        Self::build(game, config, inspection)
    }

    #[cfg(not(feature = "inspection"))]
    fn new_inner(
        game: GameEngine<G>,
        config: LogicEndpointConfig,
    ) -> Result<Self, GameEngineError> {
        Self::build(game, config)
    }

    fn build(
        game: GameEngine<G>,
        config: LogicEndpointConfig,
        #[cfg(feature = "inspection")] inspection: InspectionEndpointRuntime,
    ) -> Result<Self, GameEngineError> {
        if config.max_output_packets == 0
            || config.max_output_bytes == 0
            || config.asset_channel_epoch == 0
            || config.audio_channel_epoch == 0
            || config.render_channel_epoch == 0
            || config.max_readback_results == 0
            || config.max_readback_bytes == 0
            || config.max_readback_bytes > MAX_WIRE_READBACK_BYTES
        {
            return Err(GameEngineError::InvalidConfig);
        }
        let render_assets = RenderAssetOutbox::new(game.id(), config.render_assets)
            .map_err(|_| GameEngineError::InvalidConfig)?;
        let audio_assets = RenderAssetOutbox::new(game.id(), config.audio_assets)
            .map_err(|_| GameEngineError::InvalidConfig)?;
        #[cfg(feature = "inspection")]
        let faults = FaultInjector::new(FaultInjectionConfig::default())
            .map_err(|_| GameEngineError::InvalidConfig)?;
        Ok(Self {
            game,
            config,
            state: LogicEndpointState::Created,
            output: VecDeque::new(),
            output_bytes: 0,
            output_peak_packets: 0,
            output_peak_bytes: 0,
            output_capacity_rejections: 0,
            pending_startup_asset_controls: BTreeSet::new(),
            startup_render_control_ack: 0,
            startup_audio_control_ack: 0,
            next_haptics_sequence: 1,
            render_assets,
            audio_assets,
            readback_results: VecDeque::new(),
            readback_result_bytes: 0,
            #[cfg(feature = "inspection")]
            inspection,
            #[cfg(feature = "inspection")]
            faults,
        })
    }

    pub const fn state(&self) -> LogicEndpointState {
        self.state
    }

    pub const fn game(&self) -> &GameEngine<G> {
        &self.game
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

    pub fn owner_snapshot(&self) -> LogicEndpointDriverOwnerSnapshot {
        LogicEndpointDriverOwnerSnapshot {
            state: self.state,
            game: self.game.owner_snapshot(),
            startup_asset_controls: self.pending_startup_asset_controls.len(),
            render_assets: self.render_assets.owner_snapshot(),
            audio_assets: self.audio_assets.owner_snapshot(),
            readback_results: self.readback_results.len(),
            readback_result_bytes: self.readback_result_bytes,
            #[cfg(feature = "inspection")]
            inspection: self.inspection.owner_snapshot(),
            output: self.queue_metrics(),
        }
    }

    fn shutdown_owned_logic(
        &mut self,
    ) -> Result<LogicEndpointDriverShutdownReport, EndpointDriverError> {
        let game_before = self.game.owner_snapshot();
        match self.game.state() {
            GameEngineState::Closed => {}
            GameEngineState::Constructed => self
                .game
                .abandon()
                .map_err(|_| EndpointDriverError::Failed)?,
            _ => self
                .game
                .shutdown()
                .map_err(|_| EndpointDriverError::Failed)?,
        }
        let game_after = self.game.owner_snapshot();
        let render_assets = self.render_assets.shutdown();
        let audio_assets = self.audio_assets.shutdown();
        let released_startup_asset_controls = self.pending_startup_asset_controls.len();
        let released_readback_results = self.readback_results.len();
        let released_readback_result_bytes = self.readback_result_bytes;
        self.pending_startup_asset_controls.clear();
        self.startup_render_control_ack = 0;
        self.startup_audio_control_ack = 0;
        self.next_haptics_sequence = 1;
        self.readback_results.clear();
        self.readback_result_bytes = 0;
        #[cfg(feature = "inspection")]
        let inspection = self.inspection.shutdown();
        #[cfg(feature = "inspection")]
        self.faults.shutdown();
        Ok(LogicEndpointDriverShutdownReport {
            game_before,
            game_after,
            render_assets,
            audio_assets,
            released_startup_asset_controls,
            released_readback_results,
            released_readback_result_bytes,
            #[cfg(feature = "inspection")]
            inspection,
        })
    }

    pub fn queue_asset_request(
        &mut self,
        channel_epoch: u64,
        request_sequence: u64,
        scope: AssetScopeId,
        asset_ref: AssetRef,
        deadline_millis: u64,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if channel_epoch == 0
            || request_sequence == 0
            || scope.engine != self.game.id()
            || asset_ref.engine != self.game.id()
            || !scope.handle.is_valid()
            || !asset_ref.handle.is_valid()
        {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        let mut payload = Vec::with_capacity(24);
        write_handle(&mut payload, scope.handle);
        write_handle(&mut payload, asset_ref.handle);
        payload.extend_from_slice(&deadline_millis.to_le_bytes());
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AssetRequest,
                engine: self.game.id(),
                channel_epoch,
                commit_id: 0,
                base_revision: 0,
                new_revision: 0,
                required_control_revision: 0,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: request_sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    pub fn queue_asset_control(
        &mut self,
        channel_epoch: u64,
        command_sequence: u64,
        payload: Vec<u8>,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if channel_epoch == 0 || command_sequence == 0 || payload.is_empty() {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AssetControl,
                engine: self.game.id(),
                channel_epoch,
                commit_id: command_sequence,
                base_revision: 0,
                new_revision: 0,
                required_control_revision: 0,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: command_sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    pub fn queue_audio_control(
        &mut self,
        channel_epoch: u64,
        snapshot: &ControlStateSnapshot,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        self.ensure_output_slots(1)?;
        let bytes = encode_audio_control_snapshot(snapshot, channel_epoch)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.push(owned_from_wire(&bytes)?)
    }

    pub fn queue_render_control(
        &mut self,
        channel_epoch: u64,
        snapshot: &ControlStateSnapshot,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        self.ensure_output_slots(1)?;
        let bytes = encode_render_control_snapshot(snapshot, channel_epoch)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.push(owned_from_wire(&bytes)?)
    }

    pub fn queue_audio_event(
        &mut self,
        channel_epoch: u64,
        event: AudioEvent,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if channel_epoch == 0
            || event.engine != self.game.id()
            || event.sequence == 0
            || event.event_id == 0
            || event.tick_id == 0
            || event.deadline_millis == 0
        {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AudioEvent,
                engine: event.engine,
                channel_epoch,
                commit_id: event.event_id,
                base_revision: 0,
                new_revision: event.deadline_millis,
                required_control_revision: event.required_audio_control_revision,
                source_simulation_revision: event.tick_id,
                sequence: event.sequence,
                payload_len: event.payload.len() as u32,
            },
            payload: event.payload,
        })
    }

    pub fn queue_audio_asset_data(
        &mut self,
        asset: AssetRef,
        bytes: &[u8],
    ) -> Result<(), EndpointDriverError> {
        self.queue_audio_asset_revision(asset, 1, bytes)
    }

    pub fn queue_audio_asset_revision(
        &mut self,
        asset: AssetRef,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if asset.engine != self.game.id() {
            return Err(EndpointDriverError::Rejected);
        }
        let payload = encode_audio_asset_payload(asset, revision, bytes, false)
            .ok_or(EndpointDriverError::Rejected)?;
        self.stage_audio_asset(asset, revision, payload, false)
    }

    pub fn queue_remove_audio_asset(
        &mut self,
        asset: AssetRef,
        revision: u64,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if asset.engine != self.game.id() {
            return Err(EndpointDriverError::Rejected);
        }
        let payload = encode_audio_asset_payload(asset, revision, &[], true)
            .ok_or(EndpointDriverError::Rejected)?;
        self.stage_audio_asset(asset, revision, payload, true)
    }

    pub fn queue_render_event(
        &mut self,
        channel_epoch: u64,
        event: RenderEvent,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if channel_epoch == 0
            || event.engine != self.game.id()
            || event.sequence == 0
            || event.event_id == 0
            || event.deadline_millis == 0
        {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderEvent,
                engine: event.engine,
                channel_epoch,
                commit_id: event.event_id,
                base_revision: event.min_render_revision,
                new_revision: event.deadline_millis,
                required_control_revision: event.required_control_revision,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: event.sequence,
                payload_len: event.payload.len() as u32,
            },
            payload: event.payload,
        })
    }

    pub fn queue_render_readback(
        &mut self,
        request: ReadbackRequest,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if request.target.engine != self.game.id() {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        let payload =
            encode_readback_request(request).map_err(|_| EndpointDriverError::Rejected)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderReadbackRequest,
                engine: self.game.id(),
                channel_epoch: self.config.render_channel_epoch,
                commit_id: request.id.0,
                base_revision: request.expected_target_revision,
                new_revision: request.expected_target_revision,
                required_control_revision: request.required_control_revision,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: request.id.0,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    pub fn queue_cancel_render_readback(
        &mut self,
        id: ReadbackRequestId,
        required_control_revision: u64,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if required_control_revision == 0 {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_slots(1)?;
        let payload = encode_readback_cancel(id).map_err(|_| EndpointDriverError::Rejected)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderReadbackRequest,
                engine: self.game.id(),
                channel_epoch: self.config.render_channel_epoch,
                commit_id: id.0,
                base_revision: 0,
                new_revision: 0,
                required_control_revision,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: id.0,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    pub fn take_render_readback_result(&mut self) -> Option<ReadbackResult> {
        let result = self.readback_results.pop_front()?;
        self.readback_result_bytes = self
            .readback_result_bytes
            .saturating_sub(result.bytes.len() + result.diagnostic.len());
        Some(result)
    }

    pub fn queue_render_texture(
        &mut self,
        texture: u64,
        asset_revision: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        let byte_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(4))
            .filter(|expected| *expected == pixels.len())
            .ok_or(EndpointDriverError::Rejected)?;
        if texture == 0 || asset_revision == 0 || byte_len > u32::MAX as usize {
            return Err(EndpointDriverError::Rejected);
        }
        let mut payload = Vec::with_capacity(33 + pixels.len());
        payload.extend_from_slice(b"VRT1");
        payload.push(1);
        payload.extend_from_slice(&texture.to_le_bytes());
        payload.extend_from_slice(&asset_revision.to_le_bytes());
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&(byte_len as u32).to_le_bytes());
        payload.extend_from_slice(pixels);
        self.stage_render_asset(1, texture, asset_revision, payload, false)
    }

    pub fn queue_remove_render_texture(
        &mut self,
        texture: u64,
        asset_revision: u64,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if texture == 0 || asset_revision == 0 {
            return Err(EndpointDriverError::Rejected);
        }
        let mut payload = Vec::with_capacity(21);
        payload.extend_from_slice(b"VRT1");
        payload.push(2);
        payload.extend_from_slice(&texture.to_le_bytes());
        payload.extend_from_slice(&asset_revision.to_le_bytes());
        self.stage_render_asset(1, texture, asset_revision, payload, true)
    }

    pub fn queue_render_asset(
        &mut self,
        asset_kind: u32,
        asset: u64,
        asset_revision: u64,
        bytes: &[u8],
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if asset_kind < 2 || asset == 0 || asset_revision == 0 || bytes.is_empty() {
            return Err(EndpointDriverError::Rejected);
        }
        let byte_len = u32::try_from(bytes.len()).map_err(|_| EndpointDriverError::Rejected)?;
        let mut payload = Vec::with_capacity(29 + bytes.len());
        payload.extend_from_slice(b"VRA1");
        payload.push(1);
        payload.extend_from_slice(&asset_kind.to_le_bytes());
        payload.extend_from_slice(&asset.to_le_bytes());
        payload.extend_from_slice(&asset_revision.to_le_bytes());
        payload.extend_from_slice(&byte_len.to_le_bytes());
        payload.extend_from_slice(bytes);
        self.stage_render_asset(asset_kind, asset, asset_revision, payload, false)
    }

    pub fn queue_remove_render_asset(
        &mut self,
        asset_kind: u32,
        asset: u64,
        asset_revision: u64,
    ) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        if asset_kind < 2 || asset == 0 || asset_revision == 0 {
            return Err(EndpointDriverError::Rejected);
        }
        let mut payload = Vec::with_capacity(25);
        payload.extend_from_slice(b"VRA1");
        payload.push(2);
        payload.extend_from_slice(&asset_kind.to_le_bytes());
        payload.extend_from_slice(&asset.to_le_bytes());
        payload.extend_from_slice(&asset_revision.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        self.stage_render_asset(asset_kind, asset, asset_revision, payload, true)
    }

    pub fn begin_render_asset_recovery(&mut self) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        self.render_assets
            .begin_renderer_recovery()
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.flush_render_assets()
    }

    pub fn render_assets_synchronized(&self) -> bool {
        self.render_assets.is_synchronized()
    }

    fn stage_render_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        payload: Vec<u8>,
        tombstone: bool,
    ) -> Result<(), EndpointDriverError> {
        self.render_assets
            .stage(RenderAssetKey { kind, asset }, revision, payload, tombstone)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.flush_render_assets()
    }

    fn flush_render_assets(&mut self) -> Result<(), EndpointDriverError> {
        while self.output.len() < self.config.max_output_packets
            && self.render_assets.has_dispatch_capacity()
        {
            let Some(payload_len) = self.render_assets.next_payload_len() else {
                break;
            };
            if self
                .output_bytes
                .checked_add(payload_len)
                .is_none_or(|bytes| bytes > self.config.max_output_bytes)
            {
                break;
            }
            let Some(dispatch) = self
                .render_assets
                .poll()
                .map_err(|_| EndpointDriverError::Rejected)?
            else {
                break;
            };
            self.push(OwnedEndpointPacket {
                header: PacketHeader {
                    kind: MessageKind::RenderAssetData,
                    engine: self.game.id(),
                    channel_epoch: self.config.render_channel_epoch,
                    commit_id: dispatch.key.asset,
                    base_revision: 0,
                    new_revision: dispatch.revision,
                    required_control_revision: self.game.render_control_revision(),
                    source_simulation_revision: self.game.simulation_revision(),
                    sequence: 0,
                    payload_len: dispatch.payload.len() as u32,
                },
                payload: dispatch.payload,
            })?;
        }
        Ok(())
    }

    pub fn begin_audio_asset_recovery(&mut self) -> Result<(), EndpointDriverError> {
        self.require_owned_logic_open()?;
        self.audio_assets
            .begin_renderer_recovery()
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.flush_audio_assets()
    }

    pub fn audio_assets_synchronized(&self) -> bool {
        self.audio_assets.is_synchronized()
    }

    fn stage_audio_asset(
        &mut self,
        asset: AssetRef,
        revision: u64,
        payload: Vec<u8>,
        tombstone: bool,
    ) -> Result<(), EndpointDriverError> {
        self.audio_assets
            .stage(
                RenderAssetKey {
                    kind: 1,
                    asset: handle_key(asset.handle),
                },
                revision,
                payload,
                tombstone,
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.flush_audio_assets()
    }

    fn flush_audio_assets(&mut self) -> Result<(), EndpointDriverError> {
        while self.output.len() < self.config.max_output_packets
            && self.audio_assets.has_dispatch_capacity()
        {
            let Some(payload_len) = self.audio_assets.next_payload_len() else {
                break;
            };
            if self
                .output_bytes
                .checked_add(payload_len)
                .is_none_or(|bytes| bytes > self.config.max_output_bytes)
            {
                break;
            }
            let Some(dispatch) = self
                .audio_assets
                .poll()
                .map_err(|_| EndpointDriverError::Rejected)?
            else {
                break;
            };
            self.push(OwnedEndpointPacket {
                header: PacketHeader {
                    kind: MessageKind::AudioAssetData,
                    engine: self.game.id(),
                    channel_epoch: self.config.audio_channel_epoch,
                    commit_id: dispatch.key.asset,
                    base_revision: 0,
                    new_revision: dispatch.revision,
                    required_control_revision: self.game.audio_control_revision(),
                    source_simulation_revision: self.game.simulation_revision(),
                    sequence: 0,
                    payload_len: dispatch.payload.len() as u32,
                },
                payload: dispatch.payload,
            })?;
        }
        Ok(())
    }

    fn ensure_output_slots(&mut self, count: usize) -> Result<(), EndpointDriverError> {
        if self.output.len().saturating_add(count) > self.config.max_output_packets {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Rejected);
        }
        Ok(())
    }

    fn require_owned_logic_open(&self) -> Result<(), EndpointDriverError> {
        if matches!(
            self.state,
            LogicEndpointState::Closed | LogicEndpointState::Failed
        ) {
            Err(EndpointDriverError::Rejected)
        } else {
            Ok(())
        }
    }

    fn push(&mut self, packet: OwnedEndpointPacket) -> Result<(), EndpointDriverError> {
        packet.validate().map_err(|_| EndpointDriverError::Failed)?;
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

    fn push_batch(&mut self, packets: Vec<OwnedEndpointPacket>) -> Result<(), EndpointDriverError> {
        self.ensure_output_slots(packets.len())?;
        let added_bytes = packets.iter().try_fold(0_usize, |total, packet| {
            packet.validate().ok()?;
            total.checked_add(packet.payload.len())
        });
        if added_bytes
            .and_then(|bytes| self.output_bytes.checked_add(bytes))
            .is_none_or(|bytes| bytes > self.config.max_output_bytes)
        {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Rejected);
        }
        for packet in packets {
            self.push(packet)?;
        }
        Ok(())
    }

    fn flush_game_io(&mut self) -> Result<(), EndpointDriverError> {
        let commands = self
            .game
            .logic_io_commands()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if commands.is_empty() {
            return Ok(());
        }
        let mut next_haptics_sequence = self.next_haptics_sequence;
        let mut packets = Vec::with_capacity(commands.len());
        for command in &commands {
            packets.push(encode_logic_io_command(
                self.game.id(),
                self.game.simulation_revision(),
                self.config,
                next_haptics_sequence,
                command,
            )?);
            if matches!(command, LogicIoCommand::Haptics(_)) {
                next_haptics_sequence = next_haptics_sequence
                    .checked_add(1)
                    .ok_or(EndpointDriverError::Rejected)?;
            }
        }
        self.push_batch(packets)?;
        self.next_haptics_sequence = next_haptics_sequence;
        if self.state == LogicEndpointState::Starting {
            for command in &commands {
                if let LogicIoCommand::AssetControl { sequence, .. } = command {
                    self.pending_startup_asset_controls.insert(*sequence);
                }
            }
        }
        self.game
            .consume_logic_io(commands.len())
            .map_err(|_| EndpointDriverError::Failed)?;
        Ok(())
    }

    fn complete_start_if_ready(&mut self, source: PacketHeader) -> Result<(), EndpointDriverError> {
        if self.state != LogicEndpointState::Starting
            || !self.pending_startup_asset_controls.is_empty()
            || self.startup_render_control_ack < self.game.render_control_revision()
            || self.startup_audio_control_ack < self.game.audio_control_revision()
        {
            return Ok(());
        }
        self.state = LogicEndpointState::Running;
        self.push_empty(MessageKind::EngineReady, source)
    }

    fn push_empty(
        &mut self,
        kind: MessageKind,
        source: PacketHeader,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind,
                engine: self.game.id(),
                channel_epoch: source.channel_epoch,
                commit_id: 0,
                base_revision: 0,
                new_revision: self.game.simulation_revision(),
                required_control_revision: self.game.render_control_revision(),
                source_simulation_revision: self.game.simulation_revision(),
                sequence: source.sequence,
                payload_len: 0,
            },
            payload: Vec::new(),
        })
    }

    fn push_tick_result(
        &mut self,
        source: PacketHeader,
        tick: crate::world::SimulationTick,
    ) -> Result<(), EndpointDriverError> {
        let payload = tick.deterministic_hash.to_le_bytes().to_vec();
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::TickResult,
                engine: self.game.id(),
                channel_epoch: source.channel_epoch,
                commit_id: 0,
                base_revision: tick.world_revision,
                new_revision: tick.simulation_revision,
                required_control_revision: 0,
                source_simulation_revision: tick.simulation_revision,
                sequence: tick.tick_id,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    #[cfg(feature = "inspection")]
    fn push_inspection_response(
        &mut self,
        source: PacketHeader,
        reply: InspectionEndpointReply,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::InspectionResponse,
                engine: self.game.id(),
                channel_epoch: source.channel_epoch,
                commit_id: source.commit_id,
                base_revision: source.base_revision,
                new_revision: self.game.world().revision(),
                required_control_revision: 0,
                source_simulation_revision: self.game.simulation_revision(),
                sequence: source.sequence,
                payload_len: reply.payload.len() as u32,
            },
            payload: reply.payload,
        })
    }

    #[cfg(feature = "inspection")]
    fn apply_inspection_control(
        &mut self,
        reply: &mut InspectionEndpointReply,
    ) -> Result<(), EndpointDriverError> {
        let control = reply.control;
        if control == InspectionRequestControl::None {
            return Ok(());
        }
        let valid_state = match control {
            InspectionRequestControl::Pause { .. } => self.state == LogicEndpointState::Running,
            InspectionRequestControl::Step { .. } | InspectionRequestControl::Resume { .. } => {
                self.state == LogicEndpointState::Suspended
            }
            InspectionRequestControl::None => true,
        };
        if !valid_state {
            reply.reject_control(crate::inspection::InspectionError::InvalidExecutionState);
            return Ok(());
        }
        let revision = match self.inspection.commit_control(control) {
            Ok(revision) => revision,
            Err(error) => {
                reply.reject_control(error);
                return Ok(());
            }
        };
        match control {
            InspectionRequestControl::Pause { .. } => {
                self.game.pause().map_err(|_| EndpointDriverError::Failed)?;
                self.state = LogicEndpointState::Suspended;
            }
            InspectionRequestControl::Resume { .. } => {
                self.game
                    .resume()
                    .map_err(|_| EndpointDriverError::Failed)?;
                self.state = LogicEndpointState::Running;
            }
            InspectionRequestControl::Step { .. } | InspectionRequestControl::None => {}
        }
        reply.finish_control(revision);
        Ok(())
    }

    #[cfg(feature = "inspection")]
    fn prepare_tick(&mut self) -> Result<bool, EndpointDriverError> {
        match self.state {
            LogicEndpointState::Running => Ok(false),
            LogicEndpointState::Suspended if self.inspection.take_step() => {
                self.game
                    .resume()
                    .map_err(|_| EndpointDriverError::Failed)?;
                Ok(true)
            }
            _ => Err(EndpointDriverError::Rejected),
        }
    }

    #[cfg(not(feature = "inspection"))]
    fn prepare_tick(&mut self) -> Result<bool, EndpointDriverError> {
        if self.state == LogicEndpointState::Running {
            Ok(false)
        } else {
            Err(EndpointDriverError::Rejected)
        }
    }

    fn flush_render_output(
        &mut self,
        domain: PresentationDomainId,
    ) -> Result<(), EndpointDriverError> {
        if let Ok(transaction) = self
            .game
            .render_outbox_mut()
            .poll_durable()
            .map_err(|_| EndpointDriverError::Failed)?
        {
            let bytes =
                encode_render_transaction(&transaction).map_err(|_| EndpointDriverError::Failed)?;
            self.push(owned_from_wire(&bytes)?)?;
        }
        if let Some(transient) = self.game.render_outbox_mut().take_transient(domain) {
            let bytes =
                encode_render_transient(&transient, self.game.render_outbox().channel_epoch())
                    .map_err(|_| EndpointDriverError::Failed)?;
            self.push(owned_from_wire(&bytes)?)?;
        }
        Ok(())
    }

    fn fail<T>(&mut self) -> Result<T, EndpointDriverError> {
        self.state = LogicEndpointState::Failed;
        Err(EndpointDriverError::Failed)
    }
}

impl<G: Game> EndpointDriver for LogicEndpointDriver<G> {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError> {
        if header.engine != self.game.id() {
            return self.fail();
        }
        #[cfg(feature = "inspection")]
        if let Some(fault) = self.faults.trigger(logic_fault_point(header.kind)) {
            match fault {
                InjectedFault::RejectBeforeDispatch => {
                    return Err(EndpointDriverError::Rejected);
                }
                InjectedFault::DropLatestOnly if header.kind == MessageKind::FramePulse => {
                    return Ok(DispatchOutcome::Idle);
                }
                InjectedFault::DropLatestOnly => {
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
            MessageKind::EngineStart if self.state == LogicEndpointState::Created => {
                self.ensure_output_slots(1)?;
                if !payload.is_empty() {
                    return self.fail();
                }
                self.game.start().map_err(|_| EndpointDriverError::Failed)?;
                self.state = LogicEndpointState::Starting;
                self.flush_game_io()?;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::TickInput
                if matches!(
                    self.state,
                    LogicEndpointState::Running | LogicEndpointState::Suspended
                ) =>
            {
                self.ensure_output_slots(1)?;
                if header.sequence == 0 {
                    return self.fail();
                }
                let inspection_step = self.prepare_tick()?;
                let input = InputFrame {
                    tick_id: header.sequence,
                    bytes: payload.to_vec(),
                };
                let tick = self
                    .game
                    .manual_step(&input)
                    .map_err(|_| EndpointDriverError::Failed)?;
                if inspection_step {
                    self.game.pause().map_err(|_| EndpointDriverError::Failed)?;
                }
                self.push_tick_result(header, tick)?;
                self.flush_game_io()?;
            }
            #[cfg(feature = "inspection")]
            MessageKind::InspectionRequest
                if matches!(
                    self.state,
                    LogicEndpointState::Running | LogicEndpointState::Suspended
                ) =>
            {
                if header.sequence == 0 || header.commit_id == 0 {
                    return Err(EndpointDriverError::Rejected);
                }
                self.ensure_output_slots(1)?;
                if is_fault_wire_command(payload) {
                    let response = apply_fault_wire_command(&mut self.faults, payload)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                    self.push(fault_wire_response_packet(self.game.id(), header, response))?;
                    return Ok(DispatchOutcome::Wake);
                }
                let mut reply = self
                    .inspection
                    .dispatch(self.game.world_mut_for_inspection(), payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.apply_inspection_control(&mut reply)?;
                self.push_inspection_response(header, reply)?;
            }
            MessageKind::FramePulse if self.state == LogicEndpointState::Running => {
                self.ensure_output_slots(2)?;
                let pulse = decode_frame_pulse(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let domain = pulse.surface.domain;
                self.game
                    .presentation_pulse(pulse, None)
                    .map_err(|_| EndpointDriverError::Failed)?;
                self.flush_render_output(domain)?;
                self.flush_game_io()?;
            }
            MessageKind::RenderStateAck
                if matches!(
                    self.state,
                    LogicEndpointState::Running | LogicEndpointState::Suspended
                ) =>
            {
                let (_, channel_epoch, ack) = decode_render_ack_parts(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if channel_epoch != self.game.render_outbox().channel_epoch() {
                    return Err(EndpointDriverError::Rejected);
                }
                if self.game.render_outbox().has_snapshot_inflight() {
                    self.game
                        .render_outbox_mut()
                        .acknowledge_snapshot(ack)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                } else {
                    self.game
                        .render_outbox_mut()
                        .acknowledge(ack)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                }
            }
            MessageKind::RenderAssetAck
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                if payload.len() != 4
                    || header.channel_epoch != self.config.render_channel_epoch
                    || header.commit_id == 0
                    || header.new_revision == 0
                    || read_u32(payload, 0) == 0
                {
                    return Err(EndpointDriverError::Rejected);
                }
                let key = RenderAssetKey {
                    kind: read_u32(payload, 0),
                    asset: header.commit_id,
                };
                self.render_assets
                    .acknowledge(key, header.new_revision)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.deliver_endpoint_result(RENDER_ASSET_ACK_EVENT_KIND, header, payload)?;
                self.flush_render_assets()?;
            }
            MessageKind::AudioAssetAck
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                if !payload.is_empty()
                    || header.channel_epoch != self.config.audio_channel_epoch
                    || header.commit_id == 0
                    || header.new_revision == 0
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.audio_assets
                    .acknowledge(
                        RenderAssetKey {
                            kind: 1,
                            asset: header.commit_id,
                        },
                        header.new_revision,
                    )
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.deliver_endpoint_result(AUDIO_ASSET_ACK_EVENT_KIND, header, payload)?;
                self.flush_audio_assets()?;
            }
            MessageKind::DeviceEvent
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                match payload {
                    [3] => self.begin_render_asset_recovery()?,
                    [4] => self.begin_audio_asset_recovery()?,
                    _ => return Err(EndpointDriverError::Rejected),
                }
            }
            MessageKind::AssetCompletion
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                validate_asset_completion(payload)?;
                self.pending_startup_asset_controls.remove(&header.sequence);
                self.deliver_endpoint_result(ASSET_COMPLETION_EVENT_KIND, header, payload)?;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::AudioEventResult
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                validate_audio_result(header, payload)?;
                self.deliver_endpoint_result(AUDIO_RESULT_EVENT_KIND, header, payload)?;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::RenderEventResult
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                validate_render_result(header, payload)?;
                self.deliver_endpoint_result(RENDER_RESULT_EVENT_KIND, header, payload)?;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::RenderReadbackResult
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                if header.channel_epoch != self.config.render_channel_epoch || header.commit_id == 0
                {
                    return Err(EndpointDriverError::Rejected);
                }
                let result = decode_readback_result(payload, self.game.id())
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let result_bytes = result
                    .bytes
                    .len()
                    .checked_add(result.diagnostic.len())
                    .ok_or(EndpointDriverError::Rejected)?;
                if result.id.0 != header.commit_id
                    || result.target_revision != header.base_revision
                    || result.target_revision != header.new_revision
                    || self.readback_results.len() == self.config.max_readback_results
                    || self
                        .readback_result_bytes
                        .checked_add(result_bytes)
                        .is_none_or(|bytes| bytes > self.config.max_readback_bytes)
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.readback_result_bytes += result_bytes;
                self.readback_results.push_back(result);
            }
            MessageKind::HapticsResult
                if matches!(
                    self.state,
                    LogicEndpointState::Running | LogicEndpointState::Suspended
                ) =>
            {
                decode_haptics_result_parts(
                    header,
                    payload,
                    self.game.id(),
                    self.config.render_channel_epoch,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                self.deliver_endpoint_result(HAPTICS_RESULT_EVENT_KIND, header, payload)?;
            }
            MessageKind::RenderControlAck
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                validate_control_ack(header, payload)?;
                self.game
                    .acknowledge_control(ControlDomain::Render, header.new_revision)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.startup_render_control_ack = header.new_revision;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::AudioControlAck
                if matches!(
                    self.state,
                    LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                ) =>
            {
                validate_control_ack(header, payload)?;
                self.game
                    .acknowledge_control(ControlDomain::Audio, header.new_revision)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.startup_audio_control_ack = header.new_revision;
                self.complete_start_if_ready(header)?;
            }
            MessageKind::EngineSuspend if self.state == LogicEndpointState::Running => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.game.pause().map_err(|_| EndpointDriverError::Failed)?;
                self.state = LogicEndpointState::Suspended;
            }
            MessageKind::EngineResume if self.state == LogicEndpointState::Suspended => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.game
                    .resume()
                    .map_err(|_| EndpointDriverError::Failed)?;
                self.state = LogicEndpointState::Running;
            }
            MessageKind::EngineClose
                if matches!(
                    self.state,
                    LogicEndpointState::Created
                        | LogicEndpointState::Starting
                        | LogicEndpointState::Running
                        | LogicEndpointState::Suspended
                        | LogicEndpointState::Failed
                ) =>
            {
                self.ensure_output_slots(1)?;
                if !payload.is_empty() {
                    return self.fail();
                }
                self.shutdown_owned_logic()?;
                self.state = LogicEndpointState::Closed;
                self.push_empty(MessageKind::EngineClosed, header)?;
            }
            _ => return Err(EndpointDriverError::Rejected),
        }
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
        self.flush_render_assets()?;
        self.flush_audio_assets()?;
        Ok(packets)
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), EndpointDriverError> {
        self.shutdown_owned_logic()?;
        self.state = LogicEndpointState::Closed;
        self.output.clear();
        self.output_bytes = 0;
        Ok(())
    }
}

#[cfg(feature = "inspection")]
const fn logic_fault_point(kind: MessageKind) -> FaultPoint {
    match kind {
        MessageKind::TickInput => FaultPoint::WorkerDispatch,
        MessageKind::AssetCompletion | MessageKind::RenderAssetAck | MessageKind::AudioAssetAck => {
            FaultPoint::ResourceAcquire
        }
        MessageKind::FramePulse => FaultPoint::EndpointQueue,
        MessageKind::DeviceEvent => FaultPoint::DeviceOperation,
        MessageKind::InspectionRequest => FaultPoint::ProtocolDecode,
        MessageKind::EngineClose => FaultPoint::Shutdown,
        _ => FaultPoint::EndpointQueue,
    }
}

impl<G: Game> LogicEndpointDriver<G> {
    fn deliver_endpoint_result(
        &mut self,
        event_kind: u32,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let mut bytes = Vec::with_capacity(62 + payload.len());
        bytes.extend_from_slice(&(header.kind as u16).to_le_bytes());
        for value in [
            header.channel_epoch,
            header.commit_id,
            header.base_revision,
            header.new_revision,
            header.required_control_revision,
            header.source_simulation_revision,
            header.sequence,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);
        self.game
            .deliver_endpoint_event(SimulationEvent {
                kind: event_kind,
                bytes,
            })
            .map_err(|_| EndpointDriverError::Rejected)
    }
}

fn decode_frame_pulse(
    header: PacketHeader,
    payload: &[u8],
) -> Result<PresentationPulse, EndpointDriverError> {
    if header.sequence == 0 || payload.len() != 40 {
        return Err(EndpointDriverError::Rejected);
    }
    let surface = SurfaceHandle {
        index: read_u32(payload, 0),
        generation: read_u32(payload, 4),
    };
    let domain = Handle {
        index: read_u32(payload, 8),
        generation: read_u32(payload, 12),
    };
    let deadline_micros = read_u64(payload, 16);
    let metrics = SurfaceMetrics {
        width: read_u32(payload, 24),
        height: read_u32(payload, 28),
        scale_numerator: read_u32(payload, 32),
        scale_denominator: read_u32(payload, 36),
    };
    if !surface.is_valid() || !domain.is_valid() || !metrics.is_valid() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(PresentationPulse {
        surface: GameSurfaceId {
            engine: header.engine,
            surface,
            domain: PresentationDomainId {
                engine: header.engine,
                handle: domain,
            },
        },
        pulse_id: header.sequence,
        deadline_micros,
        metrics,
    })
}

fn owned_from_wire(bytes: &[u8]) -> Result<OwnedEndpointPacket, EndpointDriverError> {
    let (header, payload) = decode_packet(bytes).map_err(|_| EndpointDriverError::Failed)?;
    Ok(OwnedEndpointPacket {
        header,
        payload: payload.to_vec(),
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn write_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn encode_audio_asset_payload(
    asset: AssetRef,
    revision: u64,
    bytes: &[u8],
    remove: bool,
) -> Option<Vec<u8>> {
    let byte_len = u32::try_from(bytes.len()).ok()?;
    if !asset.engine.is_valid()
        || !asset.handle.is_valid()
        || revision == 0
        || (remove && !bytes.is_empty())
        || (!remove && bytes.is_empty())
    {
        return None;
    }
    let mut payload = Vec::with_capacity(25 + bytes.len());
    payload.extend_from_slice(b"VPA2");
    payload.push(if remove { 2 } else { 1 });
    write_handle(&mut payload, asset.handle);
    payload.extend_from_slice(&revision.to_le_bytes());
    payload.extend_from_slice(&byte_len.to_le_bytes());
    payload.extend_from_slice(bytes);
    Some(payload)
}

fn handle_key(handle: Handle) -> u64 {
    u64::from(handle.index) | (u64::from(handle.generation) << 32)
}

fn validate_asset_completion(payload: &[u8]) -> Result<(), EndpointDriverError> {
    let (&tag, bytes) = payload.split_first().ok_or(EndpointDriverError::Rejected)?;
    let valid_handle = |offset: usize| {
        Handle {
            index: read_u32(bytes, offset),
            generation: read_u32(bytes, offset + 4),
        }
        .is_valid()
    };
    match tag {
        1 if bytes.len() == 9 => {
            if !valid_handle(0) || !matches!(bytes[8], 1..=4) {
                return Err(EndpointDriverError::Rejected);
            }
        }
        2 if bytes.len() == 16 => {
            if !valid_handle(0) || !valid_handle(8) {
                return Err(EndpointDriverError::Rejected);
            }
        }
        3 if bytes.len() == 64 => {
            if !valid_handle(0) || !valid_handle(8) || !valid_handle(56) {
                return Err(EndpointDriverError::Rejected);
            }
        }
        4 if bytes.len() == 17 => {
            if !valid_handle(0) || !valid_handle(8) || !matches!(bytes[16], 1..=5) {
                return Err(EndpointDriverError::Rejected);
            }
        }
        5 if bytes.len() >= 4 => {
            let count = read_u32(bytes, 0) as usize;
            let expected = 4_usize
                .checked_add(count.checked_mul(8).ok_or(EndpointDriverError::Rejected)?)
                .ok_or(EndpointDriverError::Rejected)?;
            if count == 0 || bytes.len() != expected {
                return Err(EndpointDriverError::Rejected);
            }
            for offset in (4..expected).step_by(8) {
                if !valid_handle(offset) {
                    return Err(EndpointDriverError::Rejected);
                }
            }
        }
        6 if bytes.len() == 1 && matches!(bytes[0], 1..=10) => {}
        _ => return Err(EndpointDriverError::Rejected),
    }
    Ok(())
}

fn validate_audio_result(header: PacketHeader, payload: &[u8]) -> Result<(), EndpointDriverError> {
    if header.sequence == 0
        || header.commit_id == 0
        || payload.len() != 1
        || !matches!(payload[0], 1..=7)
    {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(())
}

fn validate_render_result(header: PacketHeader, payload: &[u8]) -> Result<(), EndpointDriverError> {
    if header.sequence == 0
        || header.commit_id == 0
        || payload.len() != 1
        || !matches!(payload[0], 1..=6)
    {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(())
}

fn validate_control_ack(header: PacketHeader, payload: &[u8]) -> Result<(), EndpointDriverError> {
    if header.commit_id == 0 || header.new_revision == 0 || !payload.is_empty() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(())
}

fn encode_logic_io_command(
    engine: voplay_protocol::EngineId,
    simulation_revision: u64,
    config: LogicEndpointConfig,
    haptics_sequence: u64,
    command: &LogicIoCommand,
) -> Result<OwnedEndpointPacket, EndpointDriverError> {
    match command {
        LogicIoCommand::AssetRequest {
            sequence,
            scope,
            asset_ref,
            deadline_millis,
        } => {
            let mut payload = Vec::with_capacity(24);
            write_handle(&mut payload, scope.handle);
            write_handle(&mut payload, asset_ref.handle);
            payload.extend_from_slice(&deadline_millis.to_le_bytes());
            Ok(OwnedEndpointPacket {
                header: PacketHeader {
                    kind: MessageKind::AssetRequest,
                    engine,
                    channel_epoch: config.asset_channel_epoch,
                    commit_id: 0,
                    base_revision: 0,
                    new_revision: 0,
                    required_control_revision: 0,
                    source_simulation_revision: simulation_revision,
                    sequence: *sequence,
                    payload_len: payload.len() as u32,
                },
                payload,
            })
        }
        LogicIoCommand::AssetControl { sequence, payload } => Ok(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AssetControl,
                engine,
                channel_epoch: config.asset_channel_epoch,
                commit_id: *sequence,
                base_revision: 0,
                new_revision: 0,
                required_control_revision: 0,
                source_simulation_revision: simulation_revision,
                sequence: *sequence,
                payload_len: payload.len() as u32,
            },
            payload: payload.clone(),
        }),
        LogicIoCommand::RenderControl(snapshot) => {
            let packet = encode_render_control_snapshot(snapshot, config.render_channel_epoch)
                .map_err(|_| EndpointDriverError::Rejected)?;
            owned_from_wire(&packet)
        }
        LogicIoCommand::AudioControl(snapshot) => {
            let packet = encode_audio_control_snapshot(snapshot, config.audio_channel_epoch)
                .map_err(|_| EndpointDriverError::Rejected)?;
            owned_from_wire(&packet)
        }
        LogicIoCommand::RenderEvent(event) => Ok(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderEvent,
                engine,
                channel_epoch: config.render_channel_epoch,
                commit_id: event.event_id,
                base_revision: event.min_render_revision,
                new_revision: event.deadline_millis,
                required_control_revision: event.required_control_revision,
                source_simulation_revision: simulation_revision,
                sequence: event.sequence,
                payload_len: event.payload.len() as u32,
            },
            payload: event.payload.clone(),
        }),
        LogicIoCommand::AudioEvent(event) => Ok(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AudioEvent,
                engine,
                channel_epoch: config.audio_channel_epoch,
                commit_id: event.event_id,
                base_revision: 0,
                new_revision: event.deadline_millis,
                required_control_revision: event.required_audio_control_revision,
                source_simulation_revision: event.tick_id,
                sequence: event.sequence,
                payload_len: event.payload.len() as u32,
            },
            payload: event.payload.clone(),
        }),
        LogicIoCommand::Haptics(command) => {
            let bytes = encode_haptics_command(
                engine,
                config.render_channel_epoch,
                haptics_sequence,
                simulation_revision,
                *command,
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
            owned_from_wire(&bytes)
        }
    }
}
