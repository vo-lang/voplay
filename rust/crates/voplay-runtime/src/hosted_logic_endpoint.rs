use std::collections::VecDeque;

use voplay_protocol::{generated::MessageKind, EngineId, PacketHeader};

use crate::{
    audio_control::{decode_audio_control_snapshot, AudioControlDecodeConfig},
    control::{ControlDomain, ControlStateSnapshot, EngineControlConfig, EngineControlStore},
    control_authority_wire::{
        decode_control_commit, decode_control_lease_request, decode_control_observed,
        decode_control_realization_results, encode_control_committed, encode_control_lease_granted,
        encode_control_observed_ack,
    },
    control_wire::{
        decode_audio_control_snapshot_parts, decode_render_control_snapshot_parts,
        encode_audio_control_snapshot, encode_render_control_snapshot,
    },
    endpoint::{
        DispatchOutcome, EndpointDriver, EndpointDriverError, EndpointQueueMetrics,
        OwnedEndpointPacket,
    },
    render_control::{decode_render_control_snapshot, RenderControlDecodeConfig},
};

const MAX_HOSTED_LOGIC_OUTPUT_PACKETS: usize = 4096;
const MAX_HOSTED_LOGIC_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTROL_COMMIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024 + 4096 * 48 + 20 + 16 + 256 * 16;
const MAX_CONTROL_SNAPSHOT_OUTPUT_BYTES: usize = 16 * 1024 * 1024 + 4096 * 48 + 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedLogicEndpointState {
    Created,
    Running,
    Suspended,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostedLogicEndpointOwnerSnapshot {
    pub state: HostedLogicEndpointState,
    pub pending_render_control: bool,
    pub pending_audio_control: bool,
    pub render_control_entries: usize,
    pub audio_control_entries: usize,
    pub control_store: crate::control::EngineControlOwnerSnapshot,
    pub output: EndpointQueueMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostedLogicEndpointShutdownReport {
    pub released_render_control_entries: usize,
    pub released_audio_control_entries: usize,
    pub released_pending_control_forwards: usize,
    pub control_store_before: crate::control::EngineControlOwnerSnapshot,
    pub control_store_after: crate::control::EngineControlOwnerSnapshot,
}

#[derive(Clone, Copy)]
struct PendingControlForward {
    source: PacketHeader,
    forward_ack: bool,
}

/// Lifecycle endpoint for a Vo target-island owned game-logic artifact.
///
/// Fixed ticks and their role packet batches cross the target host-request
/// channel. This endpoint owns the provider-role lifecycle and readiness ACK;
/// it deliberately has no second World or schedule implementation.
pub struct HostedLogicEndpointDriver {
    engine: EngineId,
    state: HostedLogicEndpointState,
    output: VecDeque<OwnedEndpointPacket>,
    output_bytes: usize,
    pending_render_control: Option<PendingControlForward>,
    pending_audio_control: Option<PendingControlForward>,
    render_control: Option<ControlStateSnapshot>,
    audio_control: Option<ControlStateSnapshot>,
    control_store: EngineControlStore,
}

impl HostedLogicEndpointDriver {
    pub fn new(engine: EngineId) -> Result<Self, EndpointDriverError> {
        if !engine.is_valid() {
            return Err(EndpointDriverError::Rejected);
        }
        let control_store = EngineControlStore::new(engine, EngineControlConfig::default())
            .map_err(|_| EndpointDriverError::Rejected)?;
        Ok(Self {
            engine,
            state: HostedLogicEndpointState::Created,
            output: VecDeque::new(),
            output_bytes: 0,
            pending_render_control: None,
            pending_audio_control: None,
            render_control: None,
            audio_control: None,
            control_store,
        })
    }

    pub const fn state(&self) -> HostedLogicEndpointState {
        self.state
    }

    pub const fn render_control(&self) -> Option<&ControlStateSnapshot> {
        self.render_control.as_ref()
    }

    pub const fn audio_control(&self) -> Option<&ControlStateSnapshot> {
        self.audio_control.as_ref()
    }

    pub fn owner_snapshot(&self) -> HostedLogicEndpointOwnerSnapshot {
        HostedLogicEndpointOwnerSnapshot {
            state: self.state,
            pending_render_control: self.pending_render_control.is_some(),
            pending_audio_control: self.pending_audio_control.is_some(),
            render_control_entries: self
                .render_control
                .as_ref()
                .map_or(0, |snapshot| snapshot.entries.len()),
            audio_control_entries: self
                .audio_control
                .as_ref()
                .map_or(0, |snapshot| snapshot.entries.len()),
            control_store: self.control_store.owner_snapshot(),
            output: EndpointQueueMetrics {
                packets: self.output.len(),
                peak_packets: self.output.len(),
                bytes: self.output_bytes,
                peak_bytes: self.output_bytes,
                capacity_rejections: 0,
            },
        }
    }

    fn shutdown_owned_logic(&mut self) -> HostedLogicEndpointShutdownReport {
        let released_render_control_entries = self
            .render_control
            .as_ref()
            .map_or(0, |snapshot| snapshot.entries.len());
        let released_audio_control_entries = self
            .audio_control
            .as_ref()
            .map_or(0, |snapshot| snapshot.entries.len());
        let released_pending_control_forwards = self.pending_render_control.is_some() as usize
            + self.pending_audio_control.is_some() as usize;
        let control_store_before = self.control_store.owner_snapshot();
        self.pending_render_control = None;
        self.pending_audio_control = None;
        self.render_control = None;
        self.audio_control = None;
        self.control_store.shutdown();
        HostedLogicEndpointShutdownReport {
            released_render_control_entries,
            released_audio_control_entries,
            released_pending_control_forwards,
            control_store_before,
            control_store_after: self.control_store.owner_snapshot(),
        }
    }

    fn accept_control(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
        render: bool,
    ) -> Result<(), EndpointDriverError> {
        let current_revision = if render {
            self.render_control
                .as_ref()
                .map(|snapshot| snapshot.revision)
                .unwrap_or(0)
        } else {
            self.audio_control
                .as_ref()
                .map(|snapshot| snapshot.revision)
                .unwrap_or(0)
        };
        if if render {
            self.pending_render_control.is_some()
        } else {
            self.pending_audio_control.is_some()
        } {
            return Err(EndpointDriverError::Rejected);
        }
        if (current_revision != 0
            && (source.base_revision != current_revision
                || source.new_revision != current_revision.saturating_add(1)))
            || current_revision == 0
                && (source.new_revision == 0
                    || source.base_revision.checked_add(1) != Some(source.new_revision))
        {
            return Err(EndpointDriverError::Rejected);
        }
        let snapshot = if render {
            let snapshot = decode_render_control_snapshot_parts(
                source,
                payload,
                RenderControlDecodeConfig::default().max_entries,
                RenderControlDecodeConfig::default().max_descriptor_bytes,
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
            decode_render_control_snapshot(&snapshot, RenderControlDecodeConfig::default())
                .map_err(|_| EndpointDriverError::Rejected)?;
            snapshot
        } else {
            let snapshot = decode_audio_control_snapshot_parts(
                source,
                payload,
                AudioControlDecodeConfig::default().max_entries,
                AudioControlDecodeConfig::default().max_descriptor_bytes,
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
            decode_audio_control_snapshot(&snapshot, AudioControlDecodeConfig::default())
                .map_err(|_| EndpointDriverError::Rejected)?;
            snapshot
        };
        let forward = OwnedEndpointPacket {
            header: source,
            payload: payload.to_vec(),
        };
        forward
            .validate()
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.ensure_output_capacity(1, forward.payload.len())?;
        self.control_store
            .adopt_snapshot(&snapshot)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.output_bytes += forward.payload.len();
        self.output.push_back(forward);
        if render {
            self.render_control = Some(snapshot);
            self.pending_render_control = Some(PendingControlForward {
                source,
                forward_ack: true,
            });
        } else {
            self.audio_control = Some(snapshot);
            self.pending_audio_control = Some(PendingControlForward {
                source,
                forward_ack: true,
            });
        }
        Ok(())
    }

    fn accept_control_ack(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
        domain: ControlDomain,
    ) -> Result<(), EndpointDriverError> {
        if !payload.is_empty() {
            return Err(EndpointDriverError::Rejected);
        }
        let pending = match domain {
            ControlDomain::Render => self.pending_render_control,
            ControlDomain::Audio => self.pending_audio_control,
        };
        let Some(pending) = pending else {
            let current_revision = match domain {
                ControlDomain::Render => self.control_store.render_revision(),
                ControlDomain::Audio => self.control_store.audio_revision(),
            };
            if source.new_revision == current_revision
                && source.commit_id == self.control_store.last_transaction(domain)
            {
                return Ok(());
            }
            return Err(EndpointDriverError::Rejected);
        };
        if source.engine != pending.source.engine
            || source.channel_epoch != pending.source.channel_epoch
            || source.commit_id != pending.source.commit_id
            || source.new_revision != pending.source.new_revision
        {
            return Err(EndpointDriverError::Rejected);
        }
        if pending.forward_ack {
            self.ensure_output_capacity(1, 0)?;
            self.push_packet(OwnedEndpointPacket {
                header: source,
                payload: Vec::new(),
            })?;
        }
        match domain {
            ControlDomain::Render => self.pending_render_control = None,
            ControlDomain::Audio => self.pending_audio_control = None,
        }
        Ok(())
    }

    fn push_empty(
        &mut self,
        kind: MessageKind,
        source: PacketHeader,
    ) -> Result<(), EndpointDriverError> {
        let packet = OwnedEndpointPacket {
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
        };
        self.push_packet(packet)
    }

    fn push_packet(&mut self, packet: OwnedEndpointPacket) -> Result<(), EndpointDriverError> {
        packet.validate().map_err(|_| EndpointDriverError::Failed)?;
        self.ensure_output_capacity(1, packet.payload.len())?;
        self.output_bytes += packet.payload.len();
        self.output.push_back(packet);
        Ok(())
    }

    fn ensure_output_capacity(
        &self,
        additional_packets: usize,
        additional_bytes: usize,
    ) -> Result<(), EndpointDriverError> {
        if self.output.len().saturating_add(additional_packets) > MAX_HOSTED_LOGIC_OUTPUT_PACKETS
            || self
                .output_bytes
                .checked_add(additional_bytes)
                .is_none_or(|bytes| bytes > MAX_HOSTED_LOGIC_OUTPUT_BYTES)
        {
            return Err(EndpointDriverError::Failed);
        }
        Ok(())
    }

    fn accept_lease_request(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let (producer, logic_generation, capacity) = decode_control_lease_request(source, payload)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.ensure_output_capacity(1, 36)?;
        let mut staged_store = self.control_store.clone();
        let lease = staged_store
            .issue_lease(producer, logic_generation, capacity)
            .map_err(|_| EndpointDriverError::Rejected)?;
        let granted =
            encode_control_lease_granted(source, lease).map_err(|_| EndpointDriverError::Failed)?;
        self.push_packet(granted)?;
        self.control_store = staged_store;
        Ok(())
    }

    fn accept_authority_commit(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let decoded =
            decode_control_commit(source, payload).map_err(|_| EndpointDriverError::Rejected)?;
        let domain = decoded.domain;
        if match domain {
            ControlDomain::Render => self.pending_render_control.is_some(),
            ControlDomain::Audio => self.pending_audio_control.is_some(),
        } {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_capacity(2, MAX_CONTROL_COMMIT_OUTPUT_BYTES)?;
        let mut staged_store = self.control_store.clone();
        let committed = staged_store
            .commit(decoded.builder)
            .map_err(|_| EndpointDriverError::Rejected)?
            .publish_at_safe_point(decoded.logic_generation)
            .map_err(|_| EndpointDriverError::Rejected)?;
        let completion = encode_control_committed(source, domain, &committed)
            .map_err(|_| EndpointDriverError::Failed)?;
        let snapshot = staged_store
            .snapshot(domain)
            .map_err(|_| EndpointDriverError::Failed)?;
        let packet = match domain {
            ControlDomain::Render => {
                encode_render_control_snapshot(&snapshot, source.channel_epoch)
            }
            ControlDomain::Audio => encode_audio_control_snapshot(&snapshot, source.channel_epoch),
        }
        .map_err(|_| EndpointDriverError::Failed)?;
        let (header, payload) =
            voplay_protocol::decode_packet(&packet).map_err(|_| EndpointDriverError::Failed)?;
        let control_source = header;
        self.push_packet(completion)?;
        self.push_packet(OwnedEndpointPacket {
            header,
            payload: payload.to_vec(),
        })?;
        self.control_store = staged_store;
        match domain {
            ControlDomain::Render => {
                self.render_control = Some(snapshot);
                self.pending_render_control = Some(PendingControlForward {
                    source: control_source,
                    forward_ack: true,
                });
            }
            ControlDomain::Audio => {
                self.audio_control = Some(snapshot);
                self.pending_audio_control = Some(PendingControlForward {
                    source: control_source,
                    forward_ack: true,
                });
            }
        }
        Ok(())
    }

    fn accept_realization_results(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let decoded = decode_control_realization_results(source, payload)
            .map_err(|_| EndpointDriverError::Rejected)?;
        let expected_revision = match decoded.domain {
            ControlDomain::Render => self.control_store.render_revision(),
            ControlDomain::Audio => self.control_store.audio_revision(),
        };
        if decoded.revision != expected_revision
            || source.commit_id != self.control_store.last_transaction(decoded.domain)
        {
            return Err(EndpointDriverError::Rejected);
        }
        let domain = decoded.domain;
        let mut staged_store = self.control_store.clone();
        staged_store
            .observe_endpoint_generation(domain, decoded.endpoint_generation)
            .map_err(|_| EndpointDriverError::Rejected)?;
        for (stable, outcome) in decoded.results {
            staged_store
                .record_realization(stable, decoded.endpoint_generation, outcome)
                .map_err(|_| EndpointDriverError::Rejected)?;
        }
        let snapshot = staged_store
            .snapshot(domain)
            .map_err(|_| EndpointDriverError::Failed)?;
        let current_snapshot = match domain {
            ControlDomain::Render => self.render_control.as_ref(),
            ControlDomain::Audio => self.audio_control.as_ref(),
        };
        let publishes_snapshot = current_snapshot != Some(&snapshot);
        if publishes_snapshot {
            if match domain {
                ControlDomain::Render => self.pending_render_control.is_some(),
                ControlDomain::Audio => self.pending_audio_control.is_some(),
            } {
                return Err(EndpointDriverError::Rejected);
            }
            self.ensure_output_capacity(1, MAX_CONTROL_SNAPSHOT_OUTPUT_BYTES)?;
            let packet = match domain {
                ControlDomain::Render => {
                    encode_render_control_snapshot(&snapshot, source.channel_epoch)
                }
                ControlDomain::Audio => {
                    encode_audio_control_snapshot(&snapshot, source.channel_epoch)
                }
            }
            .map_err(|_| EndpointDriverError::Failed)?;
            let (header, payload) =
                voplay_protocol::decode_packet(&packet).map_err(|_| EndpointDriverError::Failed)?;
            let control_source = header;
            self.push_packet(OwnedEndpointPacket {
                header,
                payload: payload.to_vec(),
            })?;
            match domain {
                ControlDomain::Render => {
                    self.render_control = Some(snapshot);
                    self.pending_render_control = Some(PendingControlForward {
                        source: control_source,
                        forward_ack: false,
                    });
                }
                ControlDomain::Audio => {
                    self.audio_control = Some(snapshot);
                    self.pending_audio_control = Some(PendingControlForward {
                        source: control_source,
                        forward_ack: false,
                    });
                }
            }
        }
        self.control_store = staged_store;
        Ok(())
    }

    fn accept_control_observed(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let observed =
            decode_control_observed(source, payload).map_err(|_| EndpointDriverError::Rejected)?;
        let current_revision = match observed.domain {
            ControlDomain::Render => self.control_store.render_revision(),
            ControlDomain::Audio => self.control_store.audio_revision(),
        };
        if observed.revision > current_revision {
            return Err(EndpointDriverError::Rejected);
        }
        self.ensure_output_capacity(1, 56)?;
        let mut staged_store = self.control_store.clone();
        let mut updated_snapshot = None;
        if observed.revision == current_revision {
            if observed.transaction_id != staged_store.last_transaction(observed.domain) {
                return Err(EndpointDriverError::Rejected);
            }
            staged_store
                .finalize_ready_retirements(observed.domain, observed.progress)
                .map_err(|_| EndpointDriverError::Rejected)?;
            let snapshot = staged_store
                .snapshot(observed.domain)
                .map_err(|_| EndpointDriverError::Failed)?;
            updated_snapshot = Some(snapshot);
        }
        let ack = encode_control_observed_ack(source, observed)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.push_packet(ack)?;
        self.control_store = staged_store;
        if let Some(snapshot) = updated_snapshot {
            match observed.domain {
                ControlDomain::Render => self.render_control = Some(snapshot),
                ControlDomain::Audio => self.audio_control = Some(snapshot),
            }
        }
        Ok(())
    }

    fn accept_control_state_adoption(
        &mut self,
        source: PacketHeader,
        payload: &[u8],
    ) -> Result<(), EndpointDriverError> {
        let (domain, snapshot) = match payload.get(..4) {
            Some(b"VRCS") => {
                let mut header = source;
                header.kind = MessageKind::RenderControlTransaction;
                let snapshot = decode_render_control_snapshot_parts(
                    header,
                    payload,
                    RenderControlDecodeConfig::default().max_entries,
                    RenderControlDecodeConfig::default().max_descriptor_bytes,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                decode_render_control_snapshot(&snapshot, RenderControlDecodeConfig::default())
                    .map_err(|_| EndpointDriverError::Rejected)?;
                (ControlDomain::Render, snapshot)
            }
            Some(b"VACS") => {
                let mut header = source;
                header.kind = MessageKind::AudioControlTransaction;
                let snapshot = decode_audio_control_snapshot_parts(
                    header,
                    payload,
                    AudioControlDecodeConfig::default().max_entries,
                    AudioControlDecodeConfig::default().max_descriptor_bytes,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                decode_audio_control_snapshot(&snapshot, AudioControlDecodeConfig::default())
                    .map_err(|_| EndpointDriverError::Rejected)?;
                (ControlDomain::Audio, snapshot)
            }
            _ => return Err(EndpointDriverError::Rejected),
        };
        self.ensure_output_capacity(1, 0)?;
        let mut staged_store = self.control_store.clone();
        staged_store
            .adopt_snapshot(&snapshot)
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.push_empty(MessageKind::ControlStateAdopted, source)?;
        self.control_store = staged_store;
        match domain {
            ControlDomain::Render => self.render_control = Some(snapshot),
            ControlDomain::Audio => self.audio_control = Some(snapshot),
        }
        Ok(())
    }

    fn fail<T>(&mut self) -> Result<T, EndpointDriverError> {
        self.state = HostedLogicEndpointState::Failed;
        Err(EndpointDriverError::Failed)
    }
}

impl EndpointDriver for HostedLogicEndpointDriver {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError> {
        if header.engine != self.engine {
            return self.fail();
        }
        match header.kind {
            MessageKind::EngineStart if self.state == HostedLogicEndpointState::Created => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = HostedLogicEndpointState::Running;
                self.push_empty(MessageKind::EngineReady, header)?;
            }
            MessageKind::EngineSuspend if self.state == HostedLogicEndpointState::Running => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = HostedLogicEndpointState::Suspended;
            }
            MessageKind::EngineResume if self.state == HostedLogicEndpointState::Suspended => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = HostedLogicEndpointState::Running;
            }
            MessageKind::WorkerWake
                if matches!(
                    self.state,
                    HostedLogicEndpointState::Running | HostedLogicEndpointState::Suspended
                ) =>
            {
                if !payload.is_empty() {
                    return Err(EndpointDriverError::Rejected);
                }
            }
            MessageKind::ControlLeaseRequest if self.state == HostedLogicEndpointState::Running => {
                self.accept_lease_request(header, payload)?;
            }
            MessageKind::ControlCommit if self.state == HostedLogicEndpointState::Running => {
                self.accept_authority_commit(header, payload)?;
            }
            MessageKind::ControlRealizationResult
                if self.state == HostedLogicEndpointState::Running =>
            {
                self.accept_realization_results(header, payload)?;
            }
            MessageKind::ControlObserved if self.state == HostedLogicEndpointState::Running => {
                self.accept_control_observed(header, payload)?;
            }
            MessageKind::RenderControlAck if self.state == HostedLogicEndpointState::Running => {
                self.accept_control_ack(header, payload, ControlDomain::Render)?;
            }
            MessageKind::AudioControlAck if self.state == HostedLogicEndpointState::Running => {
                self.accept_control_ack(header, payload, ControlDomain::Audio)?;
            }
            MessageKind::ControlStateAdopt if self.state == HostedLogicEndpointState::Running => {
                self.accept_control_state_adoption(header, payload)?;
            }
            MessageKind::RenderControlTransaction
                if self.state == HostedLogicEndpointState::Running =>
            {
                self.accept_control(header, payload, true)?;
            }
            MessageKind::AudioControlTransaction
                if self.state == HostedLogicEndpointState::Running =>
            {
                self.accept_control(header, payload, false)?;
            }
            MessageKind::EngineClose
                if matches!(
                    self.state,
                    HostedLogicEndpointState::Created
                        | HostedLogicEndpointState::Running
                        | HostedLogicEndpointState::Suspended
                        | HostedLogicEndpointState::Failed
                ) =>
            {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.shutdown_owned_logic();
                self.state = HostedLogicEndpointState::Closed;
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
        self.output_bytes = self.output_bytes.saturating_sub(
            packets
                .iter()
                .map(|packet| packet.payload.len())
                .sum::<usize>(),
        );
        Ok(packets)
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), EndpointDriverError> {
        self.shutdown_owned_logic();
        self.output.clear();
        self.output_bytes = 0;
        self.state = HostedLogicEndpointState::Closed;
        Ok(())
    }
}
