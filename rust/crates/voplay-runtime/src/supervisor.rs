use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::control::{
    ControlDomain, ControlStateSnapshot, EngineControlConfig, EngineControlStore,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Role {
    Logic,
    Asset,
    Render,
    Audio,
}

impl Role {
    const fn index(self) -> u32 {
        match self {
            Self::Logic => 0,
            Self::Asset => 1,
            Self::Render => 2,
            Self::Audio => 3,
        }
    }

    pub const fn shutdown_order() -> [Role; 4] {
        [Self::Audio, Self::Render, Self::Asset, Self::Logic]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupState {
    Starting,
    Running,
    Suspended,
    Recovering,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderRecoveryPhase {
    ControlSnapshot,
    RenderStateSnapshot,
    AssetResidency,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioRecoveryPhase {
    ControlSnapshot,
    AssetStreams,
    DeviceActivation,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EndpointState {
    Starting,
    Ready,
    Recovering,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointHandle {
    pub engine: EngineId,
    pub role: Role,
    pub generation: Handle,
}

#[derive(Clone, Debug)]
pub struct InstanceGroupConfig {
    pub roles: BTreeSet<Role>,
    pub allow_logic_recovery: bool,
    pub control: EngineControlConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorError {
    InvalidConfig,
    InvalidState,
    RoleDisabled,
    DuplicateRole,
    WrongEngine,
    WrongRole,
    StaleEndpoint,
    WrongActor,
    GenerationExhausted,
    WrongRecoveryPhase,
    RecoveryRevisionMismatch,
    InvalidDeadline,
}

#[derive(Clone, Copy, Debug)]
struct EndpointRecord {
    generation: u32,
    actor: Option<Handle>,
    state: EndpointState,
}

#[derive(Clone, Copy, Debug)]
struct RenderRecovery {
    endpoint: EndpointHandle,
    phase: RenderRecoveryPhase,
    control_revision: u64,
    render_revision: u64,
}

#[derive(Clone, Copy, Debug)]
struct AudioRecovery {
    endpoint: EndpointHandle,
    phase: AudioRecoveryPhase,
    control_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpiredEndpointRecovery {
    pub endpoint: EndpointHandle,
}

pub struct InstanceGroupSupervisor {
    engine: EngineId,
    config: InstanceGroupConfig,
    state: GroupState,
    endpoints: BTreeMap<Role, EndpointRecord>,
    control: EngineControlStore,
    render_recovery: Option<RenderRecovery>,
    audio_recovery: Option<AudioRecovery>,
    recovery_deadlines: BTreeMap<Role, u64>,
    resume_suspended_after_recovery: bool,
}

impl InstanceGroupSupervisor {
    pub fn new(engine: EngineId, config: InstanceGroupConfig) -> Result<Self, SupervisorError> {
        if !engine.is_valid() || config.roles.is_empty() || !config.roles.contains(&Role::Logic) {
            return Err(SupervisorError::InvalidConfig);
        }
        let control = EngineControlStore::new(engine, config.control)
            .map_err(|_| SupervisorError::InvalidConfig)?;
        Ok(Self {
            engine,
            config,
            state: GroupState::Starting,
            endpoints: BTreeMap::new(),
            control,
            render_recovery: None,
            audio_recovery: None,
            recovery_deadlines: BTreeMap::new(),
            resume_suspended_after_recovery: false,
        })
    }

    pub const fn state(&self) -> GroupState {
        self.state
    }

    pub fn control_store(&self) -> &EngineControlStore {
        &self.control
    }

    pub(crate) fn control_store_mut(&mut self) -> &mut EngineControlStore {
        &mut self.control
    }

    pub fn start_role(
        &mut self,
        role: Role,
        actor: Handle,
    ) -> Result<EndpointHandle, SupervisorError> {
        if self.state != GroupState::Starting || !actor.is_valid() {
            return Err(SupervisorError::InvalidState);
        }
        if !self.config.roles.contains(&role) {
            return Err(SupervisorError::RoleDisabled);
        }
        if self.endpoints.contains_key(&role) {
            return Err(SupervisorError::DuplicateRole);
        }
        self.endpoints.insert(
            role,
            EndpointRecord {
                generation: 1,
                actor: Some(actor),
                state: EndpointState::Starting,
            },
        );
        Ok(self.handle(role))
    }

    pub fn rollback_start_role(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        if self.state != GroupState::Starting {
            return Err(SupervisorError::InvalidState);
        }
        let record = self.validate(endpoint, actor)?;
        if record.state != EndpointState::Starting {
            return Err(SupervisorError::InvalidState);
        }
        self.endpoints.remove(&endpoint.role);
        Ok(())
    }

    pub fn mark_ready(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        if !matches!(self.state, GroupState::Starting | GroupState::Recovering) {
            return Err(SupervisorError::InvalidState);
        }
        let record = self.validate_mut(endpoint, actor)?;
        if !matches!(
            record.state,
            EndpointState::Starting | EndpointState::Recovering
        ) {
            return Err(SupervisorError::InvalidState);
        }
        if endpoint.role == Role::Render
            && self
                .render_recovery
                .is_some_and(|recovery| recovery.endpoint == endpoint)
        {
            if self.render_recovery.unwrap().phase != RenderRecoveryPhase::Ready {
                return Err(SupervisorError::WrongRecoveryPhase);
            }
            self.render_recovery = None;
        }
        if endpoint.role == Role::Audio
            && self
                .audio_recovery
                .is_some_and(|recovery| recovery.endpoint == endpoint)
        {
            if self.audio_recovery.unwrap().phase != AudioRecoveryPhase::Ready {
                return Err(SupervisorError::WrongRecoveryPhase);
            }
            self.audio_recovery = None;
        }
        let record = self.endpoints.get_mut(&endpoint.role).unwrap();
        record.state = EndpointState::Ready;
        self.recovery_deadlines.remove(&endpoint.role);
        if self.endpoints.len() == self.config.roles.len()
            && self
                .endpoints
                .values()
                .all(|endpoint| endpoint.state == EndpointState::Ready)
        {
            self.state = if self.resume_suspended_after_recovery {
                GroupState::Suspended
            } else {
                GroupState::Running
            };
            self.resume_suspended_after_recovery = false;
        }
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), SupervisorError> {
        if self.state != GroupState::Running
            || self
                .endpoints
                .values()
                .any(|endpoint| endpoint.state != EndpointState::Ready)
        {
            return Err(SupervisorError::InvalidState);
        }
        self.state = GroupState::Suspended;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), SupervisorError> {
        if self.state != GroupState::Suspended
            || self
                .endpoints
                .values()
                .any(|endpoint| endpoint.state != EndpointState::Ready)
        {
            return Err(SupervisorError::InvalidState);
        }
        self.state = GroupState::Running;
        Ok(())
    }

    pub fn validate_dispatch(
        &self,
        endpoint: EndpointHandle,
        expected_role: Role,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        if endpoint.role != expected_role {
            return Err(SupervisorError::WrongRole);
        }
        let record = self.validate(endpoint, actor)?;
        if record.state != EndpointState::Ready || self.state != GroupState::Running {
            return Err(SupervisorError::InvalidState);
        }
        Ok(())
    }

    pub fn report_fault(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<Option<EndpointHandle>, SupervisorError> {
        self.report_fault_with_deadline(endpoint, actor, 0, u64::MAX)
    }

    pub fn report_fault_with_deadline(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<Option<EndpointHandle>, SupervisorError> {
        if deadline_micros <= now_micros {
            return Err(SupervisorError::InvalidDeadline);
        }
        if !matches!(self.state, GroupState::Running | GroupState::Suspended) {
            return Err(SupervisorError::InvalidState);
        }
        let was_suspended = self.state == GroupState::Suspended;
        let endpoint_state = self.validate(endpoint, actor)?.state;
        if endpoint_state != EndpointState::Ready {
            return Err(SupervisorError::InvalidState);
        }
        if endpoint.role == Role::Logic && !self.config.allow_logic_recovery {
            self.state = GroupState::Failed;
            return Ok(None);
        }
        let replacement_generation = Handle {
            index: endpoint.generation.index,
            generation: endpoint
                .generation
                .generation
                .checked_add(1)
                .ok_or(SupervisorError::GenerationExhausted)?,
        };
        match endpoint.role {
            Role::Render => self
                .control
                .preflight_endpoint_restart(
                    ControlDomain::Render,
                    endpoint.generation,
                    replacement_generation,
                )
                .map_err(|_| SupervisorError::StaleEndpoint)?,
            Role::Audio => self
                .control
                .preflight_endpoint_restart(
                    ControlDomain::Audio,
                    endpoint.generation,
                    replacement_generation,
                )
                .map_err(|_| SupervisorError::StaleEndpoint)?,
            Role::Logic | Role::Asset => {}
        }
        let record = self.endpoints.get_mut(&endpoint.role).unwrap();
        record.generation = record
            .generation
            .checked_add(1)
            .ok_or(SupervisorError::GenerationExhausted)?;
        record.actor = None;
        record.state = EndpointState::Recovering;
        self.state = GroupState::Recovering;
        self.recovery_deadlines
            .insert(endpoint.role, deadline_micros);
        self.resume_suspended_after_recovery = was_suspended;
        let replacement = self.handle(endpoint.role);
        match endpoint.role {
            Role::Render => self
                .control
                .restart_endpoint(
                    ControlDomain::Render,
                    endpoint.generation,
                    replacement.generation,
                )
                .map_err(|_| SupervisorError::StaleEndpoint)?,
            Role::Audio => self
                .control
                .restart_endpoint(
                    ControlDomain::Audio,
                    endpoint.generation,
                    replacement.generation,
                )
                .map_err(|_| SupervisorError::StaleEndpoint)?,
            Role::Logic | Role::Asset => {}
        }
        if endpoint.role == Role::Render {
            self.render_recovery = Some(RenderRecovery {
                endpoint: replacement,
                phase: RenderRecoveryPhase::ControlSnapshot,
                control_revision: 0,
                render_revision: 0,
            });
        } else if endpoint.role == Role::Audio {
            self.audio_recovery = Some(AudioRecovery {
                endpoint: replacement,
                phase: AudioRecoveryPhase::ControlSnapshot,
                control_revision: 0,
            });
        }
        Ok(Some(replacement))
    }

    pub fn next_recovery_deadline_micros(&self) -> Option<u64> {
        self.recovery_deadlines.values().copied().min()
    }

    pub fn service_recovery_deadline(
        &mut self,
        now_micros: u64,
    ) -> Option<ExpiredEndpointRecovery> {
        let expired_role = self
            .recovery_deadlines
            .iter()
            .filter(|(_, deadline)| now_micros >= **deadline)
            .map(|(role, _)| *role)
            .next();
        let Some(role) = expired_role else {
            return None;
        };
        let endpoint = self.handle(role);
        self.state = GroupState::Failed;
        self.recovery_deadlines.clear();
        self.render_recovery = None;
        self.audio_recovery = None;
        Some(ExpiredEndpointRecovery { endpoint })
    }

    pub fn bind_replacement(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        if self.state != GroupState::Recovering {
            return Err(SupervisorError::InvalidState);
        }
        if endpoint.engine != self.engine {
            return Err(SupervisorError::WrongEngine);
        }
        let record = self
            .endpoints
            .get_mut(&endpoint.role)
            .ok_or(SupervisorError::RoleDisabled)?;
        if record.generation != endpoint.generation.generation
            || endpoint.generation.index != endpoint.role.index()
        {
            return Err(SupervisorError::StaleEndpoint);
        }
        if record.state != EndpointState::Recovering || !actor.is_valid() {
            return Err(SupervisorError::InvalidState);
        }
        record.actor = Some(actor);
        record.state = EndpointState::Starting;
        Ok(())
    }

    pub fn begin_close(&mut self) -> Result<(), SupervisorError> {
        if matches!(self.state, GroupState::Closing | GroupState::Closed) {
            return Ok(());
        }
        self.state = GroupState::Closing;
        for endpoint in self.endpoints.values_mut() {
            endpoint.state = EndpointState::Closing;
        }
        if self.endpoints.is_empty() {
            self.state = GroupState::Closed;
        }
        Ok(())
    }

    pub fn next_close_role(&self) -> Option<Role> {
        if self.state != GroupState::Closing {
            return None;
        }
        Role::shutdown_order().into_iter().find(|role| {
            self.endpoints
                .get(role)
                .is_some_and(|endpoint| endpoint.state == EndpointState::Closing)
        })
    }

    pub fn render_recovery_phase(&self, endpoint: EndpointHandle) -> Option<RenderRecoveryPhase> {
        self.render_recovery
            .filter(|recovery| recovery.endpoint == endpoint)
            .map(|recovery| recovery.phase)
    }

    pub fn render_control_snapshot(
        &self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<ControlStateSnapshot, SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .render_recovery
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != RenderRecoveryPhase::ControlSnapshot {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        self.control
            .snapshot(ControlDomain::Render)
            .map_err(|_| SupervisorError::InvalidState)
    }

    pub fn confirm_render_control_snapshot(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        revision: u64,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let expected = self
            .control
            .snapshot(ControlDomain::Render)
            .map_err(|_| SupervisorError::InvalidState)?
            .revision;
        let recovery = self
            .render_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != RenderRecoveryPhase::ControlSnapshot {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        if revision != expected {
            return Err(SupervisorError::RecoveryRevisionMismatch);
        }
        recovery.control_revision = revision;
        recovery.phase = RenderRecoveryPhase::RenderStateSnapshot;
        Ok(())
    }

    pub fn confirm_render_state_snapshot(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        render_revision: u64,
        required_control_revision: u64,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .render_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != RenderRecoveryPhase::RenderStateSnapshot {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        if required_control_revision > recovery.control_revision {
            return Err(SupervisorError::RecoveryRevisionMismatch);
        }
        recovery.render_revision = render_revision;
        recovery.phase = RenderRecoveryPhase::AssetResidency;
        Ok(())
    }

    pub fn confirm_asset_residency(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        render_revision: u64,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .render_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != RenderRecoveryPhase::AssetResidency {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        if render_revision != recovery.render_revision {
            return Err(SupervisorError::RecoveryRevisionMismatch);
        }
        recovery.phase = RenderRecoveryPhase::Ready;
        Ok(())
    }

    pub fn audio_recovery_phase(&self, endpoint: EndpointHandle) -> Option<AudioRecoveryPhase> {
        self.audio_recovery
            .filter(|recovery| recovery.endpoint == endpoint)
            .map(|recovery| recovery.phase)
    }

    pub fn audio_control_snapshot(
        &self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<ControlStateSnapshot, SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .audio_recovery
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != AudioRecoveryPhase::ControlSnapshot {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        self.control
            .snapshot(ControlDomain::Audio)
            .map_err(|_| SupervisorError::InvalidState)
    }

    pub fn confirm_audio_control_snapshot(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        revision: u64,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let expected = self
            .control
            .snapshot(ControlDomain::Audio)
            .map_err(|_| SupervisorError::InvalidState)?
            .revision;
        let recovery = self
            .audio_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != AudioRecoveryPhase::ControlSnapshot {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        if revision != expected {
            return Err(SupervisorError::RecoveryRevisionMismatch);
        }
        recovery.control_revision = revision;
        recovery.phase = AudioRecoveryPhase::AssetStreams;
        Ok(())
    }

    pub fn confirm_audio_asset_streams(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
        required_control_revision: u64,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .audio_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != AudioRecoveryPhase::AssetStreams {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        if required_control_revision > recovery.control_revision {
            return Err(SupervisorError::RecoveryRevisionMismatch);
        }
        recovery.phase = AudioRecoveryPhase::DeviceActivation;
        Ok(())
    }

    pub fn confirm_audio_device_activation(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        self.validate(endpoint, actor)?;
        let recovery = self
            .audio_recovery
            .as_mut()
            .filter(|recovery| recovery.endpoint == endpoint)
            .ok_or(SupervisorError::WrongRecoveryPhase)?;
        if recovery.phase != AudioRecoveryPhase::DeviceActivation {
            return Err(SupervisorError::WrongRecoveryPhase);
        }
        recovery.phase = AudioRecoveryPhase::Ready;
        Ok(())
    }

    pub fn mark_closed(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<(), SupervisorError> {
        if self.state != GroupState::Closing {
            return Err(SupervisorError::InvalidState);
        }
        let record = self.validate_mut(endpoint, actor)?;
        if record.state != EndpointState::Closing {
            return Err(SupervisorError::InvalidState);
        }
        record.state = EndpointState::Closed;
        if self
            .endpoints
            .values()
            .all(|endpoint| endpoint.state == EndpointState::Closed)
        {
            self.state = GroupState::Closed;
        }
        Ok(())
    }

    pub(crate) fn release_closed_owned_state(&mut self) -> Result<(), SupervisorError> {
        if self.state != GroupState::Closed {
            return Err(SupervisorError::InvalidState);
        }
        self.endpoints.clear();
        self.control.shutdown();
        self.render_recovery = None;
        self.audio_recovery = None;
        self.recovery_deadlines.clear();
        self.resume_suspended_after_recovery = false;
        Ok(())
    }

    fn validate(
        &self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<&EndpointRecord, SupervisorError> {
        if endpoint.engine != self.engine {
            return Err(SupervisorError::WrongEngine);
        }
        let record = self
            .endpoints
            .get(&endpoint.role)
            .ok_or(SupervisorError::RoleDisabled)?;
        if endpoint.generation.index != endpoint.role.index()
            || endpoint.generation.generation != record.generation
        {
            return Err(SupervisorError::StaleEndpoint);
        }
        if record.actor != Some(actor) {
            return Err(SupervisorError::WrongActor);
        }
        Ok(record)
    }

    fn validate_mut(
        &mut self,
        endpoint: EndpointHandle,
        actor: Handle,
    ) -> Result<&mut EndpointRecord, SupervisorError> {
        self.validate(endpoint, actor)?;
        Ok(self.endpoints.get_mut(&endpoint.role).unwrap())
    }

    fn handle(&self, role: Role) -> EndpointHandle {
        let record = &self.endpoints[&role];
        EndpointHandle {
            engine: self.engine,
            role,
            generation: Handle {
                index: role.index(),
                generation: record.generation,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlKind, ControlRefState, RealizationOutcome};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn config() -> InstanceGroupConfig {
        InstanceGroupConfig {
            roles: BTreeSet::from([Role::Logic, Role::Render]),
            allow_logic_recovery: false,
            control: EngineControlConfig {
                max_leases: 4,
                max_refs: 4,
                max_ops_per_transaction: 4,
                max_descriptor_bytes: 32,
                max_snapshot_bytes: 64,
            },
        }
    }

    fn running() -> (InstanceGroupSupervisor, EndpointHandle, EndpointHandle) {
        let mut supervisor = InstanceGroupSupervisor::new(handle(1), config()).unwrap();
        let logic = supervisor.start_role(Role::Logic, handle(10)).unwrap();
        let render = supervisor.start_role(Role::Render, handle(20)).unwrap();
        supervisor.mark_ready(logic, handle(10)).unwrap();
        supervisor.mark_ready(render, handle(20)).unwrap();
        (supervisor, logic, render)
    }

    #[test]
    fn endpoint_identity_binds_engine_role_generation_and_actor() {
        let (supervisor, logic, _) = running();
        assert_eq!(
            supervisor.validate_dispatch(logic, Role::Render, handle(10)),
            Err(SupervisorError::WrongRole)
        );
        assert_eq!(
            supervisor.validate_dispatch(logic, Role::Logic, handle(11)),
            Err(SupervisorError::WrongActor)
        );
        let foreign = EndpointHandle {
            engine: handle(2),
            ..logic
        };
        assert_eq!(
            supervisor.validate_dispatch(foreign, Role::Logic, handle(10)),
            Err(SupervisorError::WrongEngine)
        );
    }

    #[test]
    fn render_recovery_changes_generation_and_preserves_control_desired_state() {
        let (mut supervisor, logic, render) = running();
        let lease = supervisor
            .control_store_mut()
            .issue_lease(handle(10), logic.generation, 1)
            .unwrap();
        let mut transaction = lease.begin(ControlDomain::Render, 1, 0);
        transaction
            .create(ControlKind::RenderView, b"main".to_vec())
            .unwrap();
        let stable = supervisor
            .control_store_mut()
            .commit(transaction)
            .unwrap()
            .publish_at_safe_point(logic.generation)
            .unwrap()
            .promotions[0]
            .1;
        supervisor
            .control_store_mut()
            .begin_realization(stable)
            .unwrap();
        supervisor
            .control_store_mut()
            .record_realization(
                stable,
                render.generation,
                crate::control::RealizationOutcome::Live,
            )
            .unwrap();

        let replacement = supervisor
            .report_fault(render, handle(20))
            .unwrap()
            .unwrap();
        assert_ne!(replacement.generation, render.generation);
        assert_eq!(
            supervisor.validate_dispatch(render, Role::Render, handle(20)),
            Err(SupervisorError::StaleEndpoint)
        );
        assert_eq!(
            supervisor.control_store().state(stable),
            Some(ControlRefState::Realizing)
        );
        supervisor
            .bind_replacement(replacement, handle(21))
            .unwrap();
        assert_eq!(
            supervisor.mark_ready(replacement, handle(21)),
            Err(SupervisorError::WrongRecoveryPhase)
        );
        assert_eq!(
            supervisor.confirm_render_state_snapshot(replacement, handle(21), 8, 1),
            Err(SupervisorError::WrongRecoveryPhase)
        );
        let snapshot = supervisor
            .render_control_snapshot(replacement, handle(21))
            .unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.entries[0].stable, stable);
        supervisor
            .confirm_render_control_snapshot(replacement, handle(21), snapshot.revision)
            .unwrap();
        assert_eq!(
            supervisor.confirm_render_state_snapshot(replacement, handle(21), 8, 2),
            Err(SupervisorError::RecoveryRevisionMismatch)
        );
        supervisor
            .confirm_render_state_snapshot(replacement, handle(21), 8, 1)
            .unwrap();
        assert_eq!(
            supervisor.confirm_asset_residency(replacement, handle(21), 7),
            Err(SupervisorError::RecoveryRevisionMismatch)
        );
        supervisor
            .confirm_asset_residency(replacement, handle(21), 8)
            .unwrap();
        supervisor.mark_ready(replacement, handle(21)).unwrap();
        assert_eq!(supervisor.state(), GroupState::Running);
        supervisor
            .validate_dispatch(logic, Role::Logic, handle(10))
            .unwrap();
    }

    #[test]
    fn audio_recovery_preserves_bus_ref_and_enforces_stream_then_device_order() {
        let mut config = config();
        config.roles = BTreeSet::from([Role::Logic, Role::Audio]);
        let mut supervisor = InstanceGroupSupervisor::new(handle(1), config).unwrap();
        let logic = supervisor.start_role(Role::Logic, handle(10)).unwrap();
        let audio = supervisor.start_role(Role::Audio, handle(30)).unwrap();
        supervisor.mark_ready(logic, handle(10)).unwrap();
        supervisor.mark_ready(audio, handle(30)).unwrap();
        let lease = supervisor
            .control_store_mut()
            .issue_lease(handle(10), logic.generation, 1)
            .unwrap();
        let mut transaction = lease.begin(ControlDomain::Audio, 1, 0);
        transaction
            .create(ControlKind::AudioBus, b"music".to_vec())
            .unwrap();
        let stable = supervisor
            .control_store_mut()
            .commit(transaction)
            .unwrap()
            .publish_at_safe_point(logic.generation)
            .unwrap()
            .promotions[0]
            .1;
        supervisor
            .control_store_mut()
            .begin_realization(stable)
            .unwrap();
        supervisor
            .control_store_mut()
            .record_realization(stable, audio.generation, RealizationOutcome::Live)
            .unwrap();

        let replacement = supervisor.report_fault(audio, handle(30)).unwrap().unwrap();
        supervisor
            .bind_replacement(replacement, handle(31))
            .unwrap();
        assert_eq!(
            supervisor.mark_ready(replacement, handle(31)),
            Err(SupervisorError::WrongRecoveryPhase)
        );
        let snapshot = supervisor
            .audio_control_snapshot(replacement, handle(31))
            .unwrap();
        assert_eq!(snapshot.entries[0].stable, stable);
        assert_eq!(snapshot.entries[0].descriptor, b"music".to_vec());
        supervisor
            .confirm_audio_control_snapshot(replacement, handle(31), 1)
            .unwrap();
        assert_eq!(
            supervisor.confirm_audio_device_activation(replacement, handle(31)),
            Err(SupervisorError::WrongRecoveryPhase)
        );
        assert_eq!(
            supervisor.confirm_audio_asset_streams(replacement, handle(31), 2),
            Err(SupervisorError::RecoveryRevisionMismatch)
        );
        supervisor
            .confirm_audio_asset_streams(replacement, handle(31), 1)
            .unwrap();
        supervisor
            .confirm_audio_device_activation(replacement, handle(31))
            .unwrap();
        supervisor.mark_ready(replacement, handle(31)).unwrap();
        assert_eq!(supervisor.state(), GroupState::Running);
        assert_eq!(
            supervisor.control_store().state(stable),
            Some(ControlRefState::Realizing)
        );
    }

    #[test]
    fn logic_fault_without_snapshot_policy_fails_group_and_close_is_terminal() {
        let (mut supervisor, logic, render) = running();
        assert_eq!(supervisor.report_fault(logic, handle(10)), Ok(None));
        assert_eq!(supervisor.state(), GroupState::Failed);
        supervisor.begin_close().unwrap();
        supervisor.mark_closed(logic, handle(10)).unwrap();
        supervisor.mark_closed(render, handle(20)).unwrap();
        assert_eq!(supervisor.state(), GroupState::Closed);
    }
}
