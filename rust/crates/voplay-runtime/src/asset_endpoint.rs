use std::collections::VecDeque;

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
    asset::{
        ArtifactId, AssetId, AssetRef, AssetRegistration, AssetScopeId, AssetScopeKind,
        AssetServer, AssetServerConfig, AssetTicket, AssetTicketOutcome, AssetTicketResult,
        AssetWork,
    },
    endpoint::{
        DispatchOutcome, EndpointDriver, EndpointDriverError, EndpointQueueMetrics,
        OwnedEndpointPacket,
    },
};

const REQUEST_BYTES: usize = 24;
const WORK_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetEndpointDriverConfig {
    pub server: AssetServerConfig,
    pub max_registrations_per_control: usize,
    pub max_dependencies_per_control: usize,
    pub max_output_packets: usize,
    pub max_output_bytes: usize,
}

impl Default for AssetEndpointDriverConfig {
    fn default() -> Self {
        Self {
            server: AssetServerConfig::default(),
            max_registrations_per_control: 4096,
            max_dependencies_per_control: 65_536,
            max_output_packets: 4096,
            max_output_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetEndpointDriverState {
    Created,
    Running,
    Suspended,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetEndpointDriverOwnerSnapshot {
    pub state: AssetEndpointDriverState,
    pub server: crate::asset::AssetServerOwnerSnapshot,
    pub output: EndpointQueueMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetEndpointDriverShutdownReport {
    pub before: crate::asset::AssetServerOwnerSnapshot,
    pub after: crate::asset::AssetServerOwnerSnapshot,
    pub terminal_tickets: Vec<AssetTicketResult>,
}

enum AssetControlCommand {
    Register(Vec<AssetRegistration>),
    HotReload(AssetRegistration),
    CreateScope(AssetScopeKind),
    Cancel(AssetTicket),
    CloseScope(AssetScopeId),
    ReleaseTicket(AssetTicket),
    ReleaseScope(AssetScopeId),
    Expire(u64),
    Complete(AssetWork),
    Restart,
    Fail(AssetWork),
}

pub struct AssetEndpointDriver {
    server: AssetServer,
    config: AssetEndpointDriverConfig,
    state: AssetEndpointDriverState,
    output: VecDeque<OwnedEndpointPacket>,
    output_bytes: usize,
    output_peak_packets: usize,
    output_peak_bytes: usize,
    output_capacity_rejections: u64,
    #[cfg(feature = "inspection")]
    faults: FaultInjector,
}

impl AssetEndpointDriver {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: AssetEndpointDriverConfig,
    ) -> Result<Self, EndpointDriverError> {
        if config.max_registrations_per_control == 0
            || config.max_dependencies_per_control == 0
            || config.max_output_packets == 0
            || config.max_output_bytes == 0
        {
            return Err(EndpointDriverError::Rejected);
        }
        #[cfg(feature = "inspection")]
        let faults = FaultInjector::new(FaultInjectionConfig::default())
            .map_err(|_| EndpointDriverError::Rejected)?;
        Ok(Self {
            server: AssetServer::new(engine, endpoint_generation, config.server)
                .map_err(|_| EndpointDriverError::Rejected)?,
            config,
            state: AssetEndpointDriverState::Created,
            output: VecDeque::new(),
            output_bytes: 0,
            output_peak_packets: 0,
            output_peak_bytes: 0,
            output_capacity_rejections: 0,
            #[cfg(feature = "inspection")]
            faults,
        })
    }

    pub const fn state(&self) -> AssetEndpointDriverState {
        self.state
    }

    pub const fn server(&self) -> &AssetServer {
        &self.server
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

    pub fn owner_snapshot(&self) -> AssetEndpointDriverOwnerSnapshot {
        AssetEndpointDriverOwnerSnapshot {
            state: self.state,
            server: self.server.owner_snapshot(),
            output: self.queue_metrics(),
        }
    }

    fn shutdown_owned_asset(&mut self) -> AssetEndpointDriverShutdownReport {
        let before = self.server.owner_snapshot();
        let terminal_tickets = self.server.shutdown();
        #[cfg(feature = "inspection")]
        self.faults.shutdown();
        AssetEndpointDriverShutdownReport {
            before,
            after: self.server.owner_snapshot(),
            terminal_tickets,
        }
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

    fn push_payload(
        &mut self,
        source: PacketHeader,
        payload: Vec<u8>,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::AssetCompletion,
                engine: self.server.engine(),
                channel_epoch: source.channel_epoch,
                commit_id: source.commit_id,
                base_revision: 0,
                new_revision: self.server.inspection_revision(),
                required_control_revision: 0,
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    fn push_empty(
        &mut self,
        kind: MessageKind,
        source: PacketHeader,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind,
                engine: self.server.engine(),
                channel_epoch: source.channel_epoch,
                commit_id: source.commit_id,
                base_revision: 0,
                new_revision: self.server.inspection_revision(),
                required_control_revision: 0,
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: 0,
            },
            payload: Vec::new(),
        })
    }

    fn push_ticket(
        &mut self,
        source: PacketHeader,
        ticket: AssetTicket,
        asset_ref: AssetRef,
    ) -> Result<(), EndpointDriverError> {
        let mut payload = Vec::with_capacity(17);
        payload.push(2);
        write_handle(&mut payload, ticket.handle);
        write_handle(&mut payload, asset_ref.handle);
        self.push_payload(source, payload)
    }

    fn push_scope(
        &mut self,
        source: PacketHeader,
        scope: AssetScopeId,
        kind: AssetScopeKind,
    ) -> Result<(), EndpointDriverError> {
        let mut payload = Vec::with_capacity(10);
        payload.push(1);
        write_handle(&mut payload, scope.handle);
        payload.push(scope_kind_tag(kind));
        self.push_payload(source, payload)
    }

    fn push_results(
        &mut self,
        source: PacketHeader,
        results: Vec<AssetTicketResult>,
    ) -> Result<(), EndpointDriverError> {
        for result in results {
            let mut payload = Vec::with_capacity(18);
            payload.push(4);
            write_handle(&mut payload, result.ticket.handle);
            write_handle(&mut payload, result.asset_ref.handle);
            payload.push(ticket_outcome_tag(result.outcome));
            self.push_payload(source, payload)?;
        }
        Ok(())
    }

    fn flush_work(&mut self, source: PacketHeader) -> Result<(), EndpointDriverError> {
        while let Some(work) = self.server.poll_work() {
            let mut payload = Vec::with_capacity(1 + WORK_BYTES);
            payload.push(3);
            encode_work(&mut payload, &work);
            self.push_payload(source, payload)?;
        }
        Ok(())
    }

    fn apply_control(
        &mut self,
        source: PacketHeader,
        command: AssetControlCommand,
    ) -> Result<(), EndpointDriverError> {
        match command {
            AssetControlCommand::Register(registrations) => {
                let refs = self
                    .server
                    .register_batch(registrations)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let mut payload = Vec::with_capacity(5 + refs.len() * 8);
                payload.push(5);
                payload.extend_from_slice(&(refs.len() as u32).to_le_bytes());
                for asset_ref in refs {
                    write_handle(&mut payload, asset_ref.handle);
                }
                self.push_payload(source, payload)?;
            }
            AssetControlCommand::HotReload(registration) => {
                let asset_ref = self
                    .server
                    .hot_reload(registration)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let mut payload = Vec::with_capacity(9);
                payload.push(5);
                payload.extend_from_slice(&1_u32.to_le_bytes());
                write_handle(&mut payload, asset_ref.handle);
                self.push_payload(source, payload)?;
                self.flush_work(source)?;
            }
            AssetControlCommand::CreateScope(kind) => {
                let scope = self
                    .server
                    .create_scope(kind)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_scope(source, scope, kind)?;
            }
            AssetControlCommand::Cancel(ticket) => {
                let result = self
                    .server
                    .cancel(ticket)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_results(source, vec![result])?;
            }
            AssetControlCommand::CloseScope(scope) => {
                let results = self
                    .server
                    .close_scope(scope)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_results(source, results)?;
            }
            AssetControlCommand::ReleaseTicket(ticket) => {
                self.server
                    .release_ticket(ticket)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_control_ack(source, 6)?;
            }
            AssetControlCommand::ReleaseScope(scope) => {
                self.server
                    .release_scope(scope)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_control_ack(source, 7)?;
            }
            AssetControlCommand::Expire(now_millis) => {
                let results = self.server.expire(now_millis);
                self.push_results(source, results)?;
                self.push_control_ack(source, 8)?;
            }
            AssetControlCommand::Complete(work) => {
                let results = self
                    .server
                    .complete(&work)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_results(source, results)?;
            }
            AssetControlCommand::Restart => {
                self.server
                    .restart_endpoint()
                    .map_err(|_| EndpointDriverError::Failed)?;
                self.push_control_ack(source, 10)?;
                self.flush_work(source)?;
            }
            AssetControlCommand::Fail(work) => {
                let results = self
                    .server
                    .fail(&work)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_results(source, results)?;
            }
        }
        Ok(())
    }

    fn push_control_ack(
        &mut self,
        source: PacketHeader,
        command_tag: u8,
    ) -> Result<(), EndpointDriverError> {
        self.push_payload(source, vec![6, command_tag])
    }

    fn fail<T>(&mut self) -> Result<T, EndpointDriverError> {
        self.state = AssetEndpointDriverState::Failed;
        Err(EndpointDriverError::Failed)
    }
}

impl EndpointDriver for AssetEndpointDriver {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError> {
        if header.engine != self.server.engine() {
            return self.fail();
        }
        #[cfg(feature = "inspection")]
        if let Some(fault) = self.faults.trigger(asset_fault_point(header.kind)) {
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
            MessageKind::EngineStart if self.state == AssetEndpointDriverState::Created => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = AssetEndpointDriverState::Running;
                self.push_empty(MessageKind::EngineReady, header)?;
            }
            MessageKind::AssetRequest if self.state == AssetEndpointDriverState::Running => {
                let (scope, asset_ref, deadline) =
                    decode_request(self.server.engine(), header, payload)?;
                let ticket = self
                    .server
                    .request(scope, asset_ref, deadline)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_ticket(header, ticket, asset_ref)?;
                self.flush_work(header)?;
            }
            #[cfg(feature = "inspection")]
            MessageKind::InspectionRequest
                if matches!(
                    self.state,
                    AssetEndpointDriverState::Running | AssetEndpointDriverState::Suspended
                ) && is_fault_wire_command(payload) =>
            {
                let response = apply_fault_wire_command(&mut self.faults, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push(fault_wire_response_packet(
                    self.server.engine(),
                    header,
                    response,
                ))?;
            }
            MessageKind::AssetControl
                if matches!(
                    self.state,
                    AssetEndpointDriverState::Running | AssetEndpointDriverState::Suspended
                ) =>
            {
                let command = decode_control(self.server.engine(), payload, self.config)?;
                self.apply_control(header, command)?;
            }
            MessageKind::WorkerWake if self.state == AssetEndpointDriverState::Running => {
                if !payload.is_empty() {
                    return Err(EndpointDriverError::Rejected);
                }
                self.flush_work(header)?;
            }
            MessageKind::EngineSuspend if self.state == AssetEndpointDriverState::Running => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = AssetEndpointDriverState::Suspended;
            }
            MessageKind::EngineResume if self.state == AssetEndpointDriverState::Suspended => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = AssetEndpointDriverState::Running;
                self.flush_work(header)?;
            }
            MessageKind::EngineClose
                if matches!(
                    self.state,
                    AssetEndpointDriverState::Created
                        | AssetEndpointDriverState::Running
                        | AssetEndpointDriverState::Suspended
                        | AssetEndpointDriverState::Failed
                ) =>
            {
                if !payload.is_empty() {
                    return self.fail();
                }
                let shutdown = self.shutdown_owned_asset();
                self.push_results(header, shutdown.terminal_tickets)?;
                self.state = AssetEndpointDriverState::Closed;
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
        Ok(packets)
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), EndpointDriverError> {
        self.shutdown_owned_asset();
        self.state = AssetEndpointDriverState::Closed;
        self.output.clear();
        self.output_bytes = 0;
        Ok(())
    }
}

#[cfg(feature = "inspection")]
const fn asset_fault_point(kind: MessageKind) -> FaultPoint {
    match kind {
        MessageKind::AssetRequest | MessageKind::AssetControl => FaultPoint::ResourceAcquire,
        MessageKind::WorkerWake => FaultPoint::WorkerDispatch,
        MessageKind::EngineClose => FaultPoint::Shutdown,
        _ => FaultPoint::ProtocolDecode,
    }
}

fn decode_request(
    engine: EngineId,
    header: PacketHeader,
    payload: &[u8],
) -> Result<(AssetScopeId, AssetRef, u64), EndpointDriverError> {
    if payload.len() != REQUEST_BYTES || header.sequence == 0 {
        return Err(EndpointDriverError::Rejected);
    }
    let scope = AssetScopeId {
        engine,
        handle: read_handle(payload, 0),
    };
    let asset_ref = AssetRef {
        engine,
        handle: read_handle(payload, 8),
    };
    if !scope.handle.is_valid() || !asset_ref.handle.is_valid() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok((scope, asset_ref, read_u64(payload, 16)))
}

fn decode_control(
    engine: EngineId,
    payload: &[u8],
    config: AssetEndpointDriverConfig,
) -> Result<AssetControlCommand, EndpointDriverError> {
    let (&tag, rest) = payload.split_first().ok_or(EndpointDriverError::Rejected)?;
    match tag {
        1 => Ok(AssetControlCommand::Register(decode_registrations(
            rest, true, config,
        )?)),
        2 => {
            let mut registrations = decode_registrations(rest, false, config)?;
            if registrations.len() != 1 {
                return Err(EndpointDriverError::Rejected);
            }
            Ok(AssetControlCommand::HotReload(registrations.remove(0)))
        }
        3 if rest.len() == 1 => Ok(AssetControlCommand::CreateScope(decode_scope_kind(
            rest[0],
        )?)),
        4 if rest.len() == 8 => Ok(AssetControlCommand::Cancel(AssetTicket {
            engine,
            handle: read_handle(rest, 0),
        })),
        5 if rest.len() == 8 => Ok(AssetControlCommand::CloseScope(AssetScopeId {
            engine,
            handle: read_handle(rest, 0),
        })),
        6 if rest.len() == 8 => Ok(AssetControlCommand::ReleaseTicket(AssetTicket {
            engine,
            handle: read_handle(rest, 0),
        })),
        7 if rest.len() == 8 => Ok(AssetControlCommand::ReleaseScope(AssetScopeId {
            engine,
            handle: read_handle(rest, 0),
        })),
        8 if rest.len() == 8 => Ok(AssetControlCommand::Expire(read_u64(rest, 0))),
        9 if rest.len() == WORK_BYTES => {
            Ok(AssetControlCommand::Complete(decode_work(engine, rest)?))
        }
        10 if rest.is_empty() => Ok(AssetControlCommand::Restart),
        11 if rest.len() == WORK_BYTES => Ok(AssetControlCommand::Fail(decode_work(engine, rest)?)),
        _ => Err(EndpointDriverError::Rejected),
    }
}

fn decode_registrations(
    bytes: &[u8],
    counted: bool,
    config: AssetEndpointDriverConfig,
) -> Result<Vec<AssetRegistration>, EndpointDriverError> {
    let (count, mut offset) = if counted {
        if bytes.len() < 4 {
            return Err(EndpointDriverError::Rejected);
        }
        (read_u32(bytes, 0) as usize, 4_usize)
    } else {
        (1, 0_usize)
    };
    if count == 0 || count > config.max_registrations_per_control {
        return Err(EndpointDriverError::Rejected);
    }
    let minimum = offset
        .checked_add(
            52_usize
                .checked_mul(count)
                .ok_or(EndpointDriverError::Rejected)?,
        )
        .ok_or(EndpointDriverError::Rejected)?;
    if minimum > bytes.len() {
        return Err(EndpointDriverError::Rejected);
    }
    let mut registrations = Vec::with_capacity(count);
    let mut total_dependencies = 0_usize;
    for _ in 0..count {
        if bytes.len().saturating_sub(offset) < 52 {
            return Err(EndpointDriverError::Rejected);
        }
        let mut asset_id = [0_u8; 16];
        asset_id.copy_from_slice(&bytes[offset..offset + 16]);
        let asset_type = read_u64(bytes, offset + 16);
        let source_revision = read_u64(bytes, offset + 24);
        let mut artifact_id = [0_u8; 16];
        artifact_id.copy_from_slice(&bytes[offset + 32..offset + 48]);
        let dependency_count = read_u32(bytes, offset + 48) as usize;
        total_dependencies = total_dependencies
            .checked_add(dependency_count)
            .filter(|count| *count <= config.max_dependencies_per_control)
            .ok_or(EndpointDriverError::Rejected)?;
        offset += 52;
        let dependency_bytes = dependency_count
            .checked_mul(16)
            .ok_or(EndpointDriverError::Rejected)?;
        let end = offset
            .checked_add(dependency_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or(EndpointDriverError::Rejected)?;
        let mut dependencies = Vec::with_capacity(dependency_count);
        for raw in bytes[offset..end].chunks_exact(16) {
            let mut dependency = [0_u8; 16];
            dependency.copy_from_slice(raw);
            dependencies.push(AssetId(dependency));
        }
        registrations.push(AssetRegistration {
            asset_id: AssetId(asset_id),
            asset_type,
            source_revision,
            artifact_id: ArtifactId(artifact_id),
            dependencies,
        });
        offset = end;
    }
    if offset != bytes.len() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(registrations)
}

fn encode_work(bytes: &mut Vec<u8>, work: &AssetWork) {
    write_handle(bytes, work.ticket.handle);
    write_handle(bytes, work.asset_ref.handle);
    bytes.extend_from_slice(&work.asset_id.0);
    bytes.extend_from_slice(&work.source_revision.to_le_bytes());
    bytes.extend_from_slice(&work.artifact_id.0);
    write_handle(bytes, work.endpoint_generation);
}

fn decode_work(engine: EngineId, bytes: &[u8]) -> Result<AssetWork, EndpointDriverError> {
    let ticket = read_handle(bytes, 0);
    let asset_ref = read_handle(bytes, 8);
    let mut asset_id = [0_u8; 16];
    asset_id.copy_from_slice(&bytes[16..32]);
    let mut artifact_id = [0_u8; 16];
    artifact_id.copy_from_slice(&bytes[40..56]);
    let endpoint_generation = read_handle(bytes, 56);
    if !ticket.is_valid() || !asset_ref.is_valid() || !endpoint_generation.is_valid() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(AssetWork {
        ticket: AssetTicket {
            engine,
            handle: ticket,
        },
        asset_ref: AssetRef {
            engine,
            handle: asset_ref,
        },
        asset_id: AssetId(asset_id),
        source_revision: read_u64(bytes, 32),
        artifact_id: ArtifactId(artifact_id),
        endpoint_generation,
    })
}

fn decode_scope_kind(tag: u8) -> Result<AssetScopeKind, EndpointDriverError> {
    match tag {
        1 => Ok(AssetScopeKind::Engine),
        2 => Ok(AssetScopeKind::Scene),
        3 => Ok(AssetScopeKind::Mode),
        4 => Ok(AssetScopeKind::Temporary),
        _ => Err(EndpointDriverError::Rejected),
    }
}

fn scope_kind_tag(kind: AssetScopeKind) -> u8 {
    match kind {
        AssetScopeKind::Engine => 1,
        AssetScopeKind::Scene => 2,
        AssetScopeKind::Mode => 3,
        AssetScopeKind::Temporary => 4,
    }
}

fn ticket_outcome_tag(outcome: AssetTicketOutcome) -> u8 {
    match outcome {
        AssetTicketOutcome::Ready => 1,
        AssetTicketOutcome::Cancelled => 2,
        AssetTicketOutcome::TimedOut => 3,
        AssetTicketOutcome::Closed => 4,
        AssetTicketOutcome::Failed => 5,
    }
}

fn write_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn read_handle(bytes: &[u8], offset: usize) -> Handle {
    Handle {
        index: read_u32(bytes, offset),
        generation: read_u32(bytes, offset + 4),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
