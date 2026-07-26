use std::collections::BTreeMap;

use voplay_protocol::{encode_packet, generated::MessageKind, EngineId, Handle, PacketHeader};

use crate::{
    endpoint::{
        DispatchOutcome, EndpointArena, EndpointArenaConfig, EndpointArenaMetrics,
        EndpointDescriptor, EndpointDriver, EndpointError, EndpointInstanceHandle,
        OwnedEndpointPacket,
    },
    supervisor::{
        EndpointHandle, GroupState, InstanceGroupConfig, InstanceGroupSupervisor, Role,
        SupervisorError,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupEndpoint {
    pub supervisor: EndpointHandle,
    pub instance: EndpointInstanceHandle,
    pub actor: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointGroupOwnershipSnapshot {
    pub engine: EngineId,
    pub state: GroupState,
    pub logic: Option<GroupEndpoint>,
    pub asset: Option<GroupEndpoint>,
    pub render: Option<GroupEndpoint>,
    pub audio: Option<GroupEndpoint>,
    pub control: crate::control::EngineControlOwnerSnapshot,
    pub arena: EndpointArenaMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointGroupShutdownReport {
    pub engine: EngineId,
    pub close_order: Vec<Role>,
    pub terminal_packets: Vec<(Role, Vec<OwnedEndpointPacket>)>,
    pub final_ownership: EndpointGroupOwnershipSnapshot,
}

impl EndpointGroupOwnershipSnapshot {
    pub const fn live_roles(self) -> usize {
        self.logic.is_some() as usize
            + self.asset.is_some() as usize
            + self.render.is_some() as usize
            + self.audio.is_some() as usize
    }

    pub const fn is_fully_released(self) -> bool {
        self.live_roles() == 0
            && self.arena.live == 0
            && self.control.closed
            && self.control.leases == 0
            && self.control.render_refs == 0
            && self.control.audio_refs == 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointGroupError {
    WrongEngine,
    DuplicateRole,
    UnknownRole,
    Supervisor(SupervisorError),
    Endpoint(EndpointError),
    Protocol,
    InvalidCloseBudget,
    CloseDrainExhausted,
    MissingEngineClosed,
}

impl From<SupervisorError> for EndpointGroupError {
    fn from(error: SupervisorError) -> Self {
        Self::Supervisor(error)
    }
}

impl From<EndpointError> for EndpointGroupError {
    fn from(error: EndpointError) -> Self {
        Self::Endpoint(error)
    }
}

pub struct EndpointGroup {
    engine: EngineId,
    supervisor: InstanceGroupSupervisor,
    arena: EndpointArena,
    endpoints: BTreeMap<Role, GroupEndpoint>,
}

impl EndpointGroup {
    pub fn new(
        engine: EngineId,
        group: InstanceGroupConfig,
        arena: EndpointArenaConfig,
    ) -> Result<Self, EndpointGroupError> {
        Ok(Self {
            engine,
            supervisor: InstanceGroupSupervisor::new(engine, group)?,
            arena: EndpointArena::new(arena)?,
            endpoints: BTreeMap::new(),
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn state(&self) -> GroupState {
        self.supervisor.state()
    }

    pub fn supervisor(&self) -> &InstanceGroupSupervisor {
        &self.supervisor
    }

    pub fn start_role(
        &mut self,
        descriptor: EndpointDescriptor,
        driver: Box<dyn EndpointDriver>,
    ) -> Result<GroupEndpoint, EndpointGroupError> {
        if descriptor.engine != self.engine {
            return Err(EndpointGroupError::WrongEngine);
        }
        if self.endpoints.contains_key(&descriptor.role) {
            return Err(EndpointGroupError::DuplicateRole);
        }
        let supervisor = self
            .supervisor
            .start_role(descriptor.role, descriptor.actor)?;
        let instance = match self.arena.create(descriptor, driver) {
            Ok(instance) => instance,
            Err(error) => {
                self.supervisor
                    .rollback_start_role(supervisor, descriptor.actor)
                    .expect(
                        "fresh supervisor start can be rolled back before any state transition",
                    );
                return Err(EndpointGroupError::Endpoint(error));
            }
        };
        let endpoint = GroupEndpoint {
            supervisor,
            instance,
            actor: descriptor.actor,
        };
        self.endpoints.insert(descriptor.role, endpoint);
        Ok(endpoint)
    }

    pub fn mark_ready(&mut self, role: Role) -> Result<(), EndpointGroupError> {
        let endpoint = self.endpoint(role)?;
        self.supervisor
            .mark_ready(endpoint.supervisor, endpoint.actor)?;
        Ok(())
    }

    pub fn dispatch(
        &mut self,
        role: Role,
        packet: &[u8],
    ) -> Result<DispatchOutcome, EndpointGroupError> {
        let endpoint = self.endpoint(role)?;
        self.supervisor
            .validate_dispatch(endpoint.supervisor, role, endpoint.actor)?;
        Ok(self
            .arena
            .dispatch(endpoint.instance, endpoint.actor, packet)?)
    }

    pub fn signal_wake(&mut self, role: Role) -> Result<(), EndpointGroupError> {
        let endpoint = self.endpoint(role)?;
        self.supervisor
            .validate_dispatch(endpoint.supervisor, role, endpoint.actor)?;
        self.arena.signal_wake(endpoint.instance, endpoint.actor)?;
        Ok(())
    }

    pub fn poll_after_wake(
        &mut self,
        role: Role,
        budget: usize,
        manual: bool,
    ) -> Result<Vec<OwnedEndpointPacket>, EndpointGroupError> {
        let endpoint = self.endpoint(role)?;
        self.supervisor
            .validate_dispatch(endpoint.supervisor, role, endpoint.actor)?;
        Ok(self
            .arena
            .poll_after_wake(endpoint.instance, endpoint.actor, budget, manual)?)
    }

    pub fn begin_close(&mut self) -> Result<(), EndpointGroupError> {
        self.supervisor.begin_close()?;
        Ok(())
    }

    pub fn close_role(
        &mut self,
        role: Role,
        deadline_micros: u64,
    ) -> Result<(), EndpointGroupError> {
        let endpoint = self.endpoint(role)?;
        self.arena
            .preflight_close_and_destroy(endpoint.instance, endpoint.actor)?;
        self.arena
            .close(endpoint.instance, endpoint.actor, deadline_micros)?;
        self.supervisor
            .mark_closed(endpoint.supervisor, endpoint.actor)?;
        self.arena.destroy(endpoint.instance, endpoint.actor)?;
        self.endpoints.remove(&role);
        Ok(())
    }

    pub fn close_all(
        &mut self,
        deadline_micros: u64,
    ) -> Result<EndpointGroupShutdownReport, EndpointGroupError> {
        if self.supervisor.state() != GroupState::Closing {
            self.begin_close()?;
        }
        let mut close_order = Vec::new();
        while let Some(role) = self.supervisor.next_close_role() {
            self.close_role(role, deadline_micros)?;
            close_order.push(role);
        }
        self.supervisor.release_closed_owned_state()?;
        Ok(EndpointGroupShutdownReport {
            engine: self.engine,
            close_order,
            terminal_packets: Vec::new(),
            final_ownership: self.ownership_snapshot(),
        })
    }

    pub fn graceful_close_all(
        &mut self,
        channel_epoch: u64,
        deadline_micros: u64,
        poll_budget: usize,
        max_poll_rounds: usize,
    ) -> Result<EndpointGroupShutdownReport, EndpointGroupError> {
        if channel_epoch == 0 || poll_budget == 0 || max_poll_rounds == 0 {
            return Err(EndpointGroupError::InvalidCloseBudget);
        }
        if self.supervisor.state() != GroupState::Closing {
            self.begin_close()?;
        }
        let mut close_order = Vec::new();
        let mut terminal_packets = Vec::new();
        let mut sequence = 1_u64;
        while let Some(role) = self.supervisor.next_close_role() {
            let endpoint = self.endpoint(role)?;
            let mut role_packets = Vec::new();
            let mut drained_before_close = false;
            for _ in 0..max_poll_rounds {
                let packets = self.arena.poll_after_wake(
                    endpoint.instance,
                    endpoint.actor,
                    poll_budget,
                    true,
                )?;
                let drained = packets.len() < poll_budget;
                role_packets.extend(packets);
                if drained {
                    drained_before_close = true;
                    break;
                }
            }
            if !drained_before_close {
                return Err(EndpointGroupError::CloseDrainExhausted);
            }
            let wire = encode_packet(
                PacketHeader {
                    kind: MessageKind::EngineClose,
                    engine: self.engine,
                    channel_epoch,
                    commit_id: 0,
                    base_revision: 0,
                    new_revision: 0,
                    required_control_revision: 0,
                    source_simulation_revision: 0,
                    sequence,
                    payload_len: 0,
                },
                &[],
            )
            .map_err(|_| EndpointGroupError::Protocol)?;
            self.arena
                .dispatch(endpoint.instance, endpoint.actor, &wire)?;
            let mut closed = false;
            for _ in 0..max_poll_rounds {
                let packets = self.arena.poll_after_wake(
                    endpoint.instance,
                    endpoint.actor,
                    poll_budget,
                    true,
                )?;
                closed |= packets
                    .iter()
                    .any(|packet| packet.header.kind == MessageKind::EngineClosed);
                let drained = packets.len() < poll_budget;
                role_packets.extend(packets);
                if closed && drained {
                    break;
                }
            }
            if !closed {
                return Err(EndpointGroupError::MissingEngineClosed);
            }
            self.close_role(role, deadline_micros)?;
            terminal_packets.push((role, role_packets));
            close_order.push(role);
            sequence = sequence
                .checked_add(1)
                .ok_or(EndpointGroupError::Protocol)?;
        }
        self.supervisor.release_closed_owned_state()?;
        Ok(EndpointGroupShutdownReport {
            engine: self.engine,
            close_order,
            terminal_packets,
            final_ownership: self.ownership_snapshot(),
        })
    }

    pub fn live_roles(&self) -> impl Iterator<Item = Role> + '_ {
        self.endpoints.keys().copied()
    }

    pub fn ownership_snapshot(&self) -> EndpointGroupOwnershipSnapshot {
        EndpointGroupOwnershipSnapshot {
            engine: self.engine,
            state: self.supervisor.state(),
            logic: self.endpoints.get(&Role::Logic).copied(),
            asset: self.endpoints.get(&Role::Asset).copied(),
            render: self.endpoints.get(&Role::Render).copied(),
            audio: self.endpoints.get(&Role::Audio).copied(),
            control: self.supervisor.control_store().owner_snapshot(),
            arena: self.arena.metrics(),
        }
    }

    fn endpoint(&self, role: Role) -> Result<GroupEndpoint, EndpointGroupError> {
        self.endpoints
            .get(&role)
            .copied()
            .ok_or(EndpointGroupError::UnknownRole)
    }
}
