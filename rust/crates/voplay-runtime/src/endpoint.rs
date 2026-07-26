use std::cell::Cell;

use voplay_protocol::{
    decode_packet, generated::MessageKind, DecodeError, EngineId, Handle, PacketHeader,
};

use crate::supervisor::Role;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EndpointInstanceHandle {
    pub engine: EngineId,
    pub role: Role,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointDescriptor {
    pub engine: EngineId,
    pub role: Role,
    pub actor: Handle,
    pub artifact_digest: [u8; 32],
    pub schema_fingerprint: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointArenaConfig {
    pub max_endpoints: usize,
    pub max_poll_packets: usize,
    pub max_poll_bytes: usize,
}

impl Default for EndpointArenaConfig {
    fn default() -> Self {
        Self {
            max_endpoints: 64,
            max_poll_packets: 4096,
            max_poll_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointState {
    Running,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchOutcome {
    Idle,
    Wake,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedEndpointPacket {
    pub header: PacketHeader,
    pub payload: Vec<u8>,
}

impl OwnedEndpointPacket {
    pub fn validate(&self) -> Result<(), EndpointError> {
        if self.payload.len() != self.header.payload_len as usize
            || self.payload.len()
                > voplay_protocol::generated::MAX_PACKET_BYTES
                    .saturating_sub(voplay_protocol::HEADER_BYTES)
        {
            return Err(EndpointError::PacketCapacity);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointDriverError {
    Rejected,
    Failed,
}

pub trait EndpointDriver {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError>;

    fn poll(&mut self, budget: usize) -> Result<Vec<OwnedEndpointPacket>, EndpointDriverError>;

    fn close(&mut self, deadline_micros: u64) -> Result<(), EndpointDriverError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointError {
    InvalidConfig,
    InvalidDescriptor,
    EndpointCapacity,
    InvalidHandle,
    StaleHandle,
    WrongActor,
    WrongEngine,
    WrongRole,
    InvalidState,
    WakeRequired,
    PacketCapacity,
    PollCapacity,
    GenerationExhausted,
    Decode(DecodeError),
    Driver(EndpointDriverError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EndpointArenaMetrics {
    pub live: usize,
    pub peak_live: usize,
    pub created: u64,
    pub destroyed: u64,
    pub dispatched_packets: u64,
    pub polled_packets: u64,
    pub polled_bytes: u64,
    pub stale_rejections: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EndpointQueueMetrics {
    pub packets: usize,
    pub peak_packets: usize,
    pub bytes: usize,
    pub peak_bytes: usize,
    pub capacity_rejections: u64,
}

impl From<DecodeError> for EndpointError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl From<EndpointDriverError> for EndpointError {
    fn from(error: EndpointDriverError) -> Self {
        Self::Driver(error)
    }
}

struct EndpointRecord {
    descriptor: EndpointDescriptor,
    state: EndpointState,
    wake_pending: bool,
    driver: Box<dyn EndpointDriver>,
}

struct EndpointSlot {
    generation: u32,
    record: Option<EndpointRecord>,
}

pub struct EndpointArena {
    config: EndpointArenaConfig,
    slots: Vec<EndpointSlot>,
    free: Vec<u32>,
    live: usize,
    metrics: Cell<EndpointArenaMetrics>,
}

impl EndpointArena {
    pub fn new(config: EndpointArenaConfig) -> Result<Self, EndpointError> {
        if config.max_endpoints == 0
            || config.max_endpoints > u32::MAX as usize
            || config.max_poll_packets == 0
            || config.max_poll_bytes == 0
        {
            return Err(EndpointError::InvalidConfig);
        }
        Ok(Self {
            config,
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            metrics: Cell::new(EndpointArenaMetrics::default()),
        })
    }

    pub fn create(
        &mut self,
        descriptor: EndpointDescriptor,
        driver: Box<dyn EndpointDriver>,
    ) -> Result<EndpointInstanceHandle, EndpointError> {
        if !descriptor.engine.is_valid()
            || !descriptor.actor.is_valid()
            || descriptor.artifact_digest.iter().all(|byte| *byte == 0)
            || descriptor.schema_fingerprint.iter().all(|byte| *byte == 0)
        {
            return Err(EndpointError::InvalidDescriptor);
        }
        if self.live == self.config.max_endpoints {
            return Err(EndpointError::EndpointCapacity);
        }
        let index = self.free.pop().unwrap_or(self.slots.len() as u32);
        if index as usize == self.slots.len() {
            self.slots.push(EndpointSlot {
                generation: 1,
                record: None,
            });
        }
        let slot = &mut self.slots[index as usize];
        let handle = EndpointInstanceHandle {
            engine: descriptor.engine,
            role: descriptor.role,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.record = Some(EndpointRecord {
            descriptor,
            state: EndpointState::Running,
            wake_pending: false,
            driver,
        });
        self.live += 1;
        let mut metrics = self.metrics.get();
        metrics.live = self.live;
        metrics.peak_live = metrics.peak_live.max(self.live);
        metrics.created = metrics.created.saturating_add(1);
        self.metrics.set(metrics);
        Ok(handle)
    }

    pub fn state(&self, handle: EndpointInstanceHandle) -> Result<EndpointState, EndpointError> {
        Ok(self.record(handle)?.state)
    }

    pub fn dispatch(
        &mut self,
        handle: EndpointInstanceHandle,
        actor: Handle,
        packet: &[u8],
    ) -> Result<DispatchOutcome, EndpointError> {
        let (header, payload) = decode_packet(packet)?;
        let record = self.record_mut(handle)?;
        validate_call(record, handle, actor)?;
        if !allows_inbound(handle.role, header.kind) {
            return Err(EndpointError::WrongRole);
        }
        if header.engine != handle.engine {
            return Err(EndpointError::WrongEngine);
        }
        let outcome = record.driver.dispatch(header, payload)?;
        if outcome == DispatchOutcome::Wake {
            record.wake_pending = true;
        }
        let mut metrics = self.metrics.get();
        metrics.dispatched_packets = metrics.dispatched_packets.saturating_add(1);
        self.metrics.set(metrics);
        Ok(outcome)
    }

    pub fn signal_wake(
        &mut self,
        handle: EndpointInstanceHandle,
        actor: Handle,
    ) -> Result<(), EndpointError> {
        let record = self.record_mut(handle)?;
        validate_call(record, handle, actor)?;
        record.wake_pending = true;
        Ok(())
    }

    pub fn poll_after_wake(
        &mut self,
        handle: EndpointInstanceHandle,
        actor: Handle,
        budget: usize,
        manual: bool,
    ) -> Result<Vec<OwnedEndpointPacket>, EndpointError> {
        let max_packets = self.config.max_poll_packets;
        let max_bytes = self.config.max_poll_bytes;
        if budget == 0 || budget > max_packets {
            return Err(EndpointError::PollCapacity);
        }
        let record = self.record_mut(handle)?;
        validate_call(record, handle, actor)?;
        if !record.wake_pending && !manual {
            return Err(EndpointError::WakeRequired);
        }
        let packets = record.driver.poll(budget)?;
        if packets.len() > budget {
            return Err(EndpointError::PollCapacity);
        }
        let mut bytes = 0usize;
        for packet in &packets {
            packet.validate()?;
            if packet.header.engine != handle.engine {
                return Err(EndpointError::WrongEngine);
            }
            if !allows_outbound(handle.role, packet.header.kind) {
                return Err(EndpointError::WrongRole);
            }
            bytes = bytes
                .checked_add(packet.payload.len())
                .filter(|bytes| *bytes <= max_bytes)
                .ok_or(EndpointError::PollCapacity)?;
        }
        record.wake_pending = packets.len() == budget;
        let mut metrics = self.metrics.get();
        metrics.polled_packets = metrics.polled_packets.saturating_add(packets.len() as u64);
        metrics.polled_bytes = metrics.polled_bytes.saturating_add(bytes as u64);
        self.metrics.set(metrics);
        Ok(packets)
    }

    pub fn close(
        &mut self,
        handle: EndpointInstanceHandle,
        actor: Handle,
        deadline_micros: u64,
    ) -> Result<(), EndpointError> {
        let record = self.record_mut(handle)?;
        validate_call(record, handle, actor)?;
        record.state = EndpointState::Closing;
        if let Err(error) = record.driver.close(deadline_micros) {
            record.state = EndpointState::Running;
            return Err(EndpointError::Driver(error));
        }
        record.state = EndpointState::Closed;
        record.wake_pending = false;
        Ok(())
    }

    pub fn preflight_close_and_destroy(
        &self,
        handle: EndpointInstanceHandle,
        actor: Handle,
    ) -> Result<(), EndpointError> {
        let record = self.record(handle)?;
        validate_call(record, handle, actor)?;
        self.slots[handle.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(EndpointError::GenerationExhausted)?;
        Ok(())
    }

    pub fn destroy(
        &mut self,
        handle: EndpointInstanceHandle,
        actor: Handle,
    ) -> Result<(), EndpointError> {
        self.preflight_destroy(handle, actor)?;
        let slot = &mut self.slots[handle.handle.index as usize];
        let generation = slot.generation + 1;
        slot.record = None;
        slot.generation = generation;
        self.free.push(handle.handle.index);
        self.live -= 1;
        let mut metrics = self.metrics.get();
        metrics.live = self.live;
        metrics.destroyed = metrics.destroyed.saturating_add(1);
        self.metrics.set(metrics);
        Ok(())
    }

    pub fn preflight_destroy(
        &self,
        handle: EndpointInstanceHandle,
        actor: Handle,
    ) -> Result<(), EndpointError> {
        let record = self.record(handle)?;
        if record.descriptor.actor != actor {
            return Err(EndpointError::WrongActor);
        }
        if record.state != EndpointState::Closed {
            return Err(EndpointError::InvalidState);
        }
        self.slots[handle.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(EndpointError::GenerationExhausted)?;
        Ok(())
    }

    pub fn metrics(&self) -> EndpointArenaMetrics {
        self.metrics.get()
    }

    fn record(&self, handle: EndpointInstanceHandle) -> Result<&EndpointRecord, EndpointError> {
        let index = self.record_index(handle)?;
        Ok(self.slots[index].record.as_ref().unwrap())
    }

    fn record_mut(
        &mut self,
        handle: EndpointInstanceHandle,
    ) -> Result<&mut EndpointRecord, EndpointError> {
        let index = self.record_index(handle)?;
        Ok(self.slots[index].record.as_mut().unwrap())
    }

    fn record_index(&self, handle: EndpointInstanceHandle) -> Result<usize, EndpointError> {
        let result = if !handle.engine.is_valid() || !handle.handle.is_valid() {
            Err(EndpointError::InvalidHandle)
        } else if let Some(slot) = self.slots.get(handle.handle.index as usize) {
            if slot.generation != handle.handle.generation || slot.record.is_none() {
                Err(EndpointError::StaleHandle)
            } else {
                let record = slot.record.as_ref().unwrap();
                if record.descriptor.engine != handle.engine {
                    Err(EndpointError::WrongEngine)
                } else if record.descriptor.role != handle.role {
                    Err(EndpointError::WrongRole)
                } else {
                    Ok(handle.handle.index as usize)
                }
            }
        } else {
            Err(EndpointError::InvalidHandle)
        };
        if result.is_err() {
            let mut metrics = self.metrics.get();
            metrics.stale_rejections = metrics.stale_rejections.saturating_add(1);
            self.metrics.set(metrics);
        }
        result
    }
}

fn validate_call(
    record: &EndpointRecord,
    handle: EndpointInstanceHandle,
    actor: Handle,
) -> Result<(), EndpointError> {
    if record.descriptor.actor != actor {
        return Err(EndpointError::WrongActor);
    }
    if record.descriptor.engine != handle.engine {
        return Err(EndpointError::WrongEngine);
    }
    if record.descriptor.role != handle.role {
        return Err(EndpointError::WrongRole);
    }
    if record.state != EndpointState::Running {
        return Err(EndpointError::InvalidState);
    }
    Ok(())
}

fn allows_inbound(role: Role, kind: MessageKind) -> bool {
    use MessageKind::*;
    match role {
        Role::Logic => matches!(
            kind,
            EngineStart
                | EngineSuspend
                | EngineResume
                | EngineClose
                | TickInput
                | FramePulse
                | AssetCompletion
                | RenderStateAck
                | RenderEventResult
                | RenderControlAck
                | RenderAssetAck
                | AudioControlAck
                | AudioEventResult
                | AudioAssetAck
                | PhysicsResult
                | HapticsResult
                | RenderReadbackResult
                | InspectionRequest
                | DeviceEvent
                | WorkerWake
                | ControlLeaseRequest
                | ControlLeaseGranted
                | ControlCommit
                | ControlCommitted
                | ControlRealizationResult
                | ControlObserved
                | ControlStateAdopt
        ),
        Role::Render => matches!(
            kind,
            EngineStart
                | EngineSuspend
                | EngineResume
                | EngineClose
                | RenderStateTransaction
                | RenderStateSnapshot
                | RenderEvent
                | RenderControlTransaction
                | RenderTransient
                | RenderAssetData
                | RenderReadbackRequest
                | InspectionRequest
                | FramePulse
                | SurfaceControl
                | DeviceEvent
                | WorkerWake
                | ControlObservedAck
        ),
        Role::Asset => matches!(
            kind,
            EngineStart
                | EngineSuspend
                | EngineResume
                | EngineClose
                | AssetRequest
                | AssetControl
                | InspectionRequest
                | DeviceEvent
                | WorkerWake
        ),
        Role::Audio => matches!(
            kind,
            EngineStart
                | EngineSuspend
                | EngineResume
                | EngineClose
                | AudioControlTransaction
                | AudioEvent
                | AudioAssetData
                | InspectionRequest
                | DeviceEvent
                | WorkerWake
                | ControlObservedAck
        ),
    }
}

fn allows_outbound(role: Role, kind: MessageKind) -> bool {
    use MessageKind::*;
    match role {
        Role::Logic => matches!(
            kind,
            EngineReady
                | EngineClosed
                | TickResult
                | RenderStateTransaction
                | RenderStateSnapshot
                | RenderEvent
                | RenderControlTransaction
                | RenderTransient
                | RenderAssetData
                | AudioControlTransaction
                | AudioEvent
                | AudioAssetData
                | AssetRequest
                | AssetControl
                | PhysicsBatch
                | Diagnostics
                | InspectionResponse
                | HapticsCommand
                | RenderReadbackRequest
                | ControlLeaseRequest
                | ControlLeaseGranted
                | ControlCommit
                | ControlCommitted
                | ControlObservedAck
                | ControlStateAdopted
                | RenderControlAck
                | AudioControlAck
        ),
        Role::Render => matches!(
            kind,
            EngineReady
                | EngineClosed
                | RenderStateAck
                | RenderEventResult
                | RenderControlAck
                | RenderAssetAck
                | RenderReadbackResult
                | ControlRealizationResult
                | ControlObserved
                | Diagnostics
                | InspectionResponse
        ),
        Role::Asset => matches!(
            kind,
            EngineReady | EngineClosed | AssetCompletion | Diagnostics | InspectionResponse
        ),
        Role::Audio => matches!(
            kind,
            EngineReady
                | EngineClosed
                | AudioControlAck
                | AudioEventResult
                | AudioAssetAck
                | ControlRealizationResult
                | ControlObserved
                | Diagnostics
                | InspectionResponse
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[derive(Default)]
    struct DriverState {
        close_fails: bool,
        closes: usize,
    }

    struct Driver {
        state: Rc<RefCell<DriverState>>,
    }

    impl EndpointDriver for Driver {
        fn dispatch(
            &mut self,
            _header: PacketHeader,
            _payload: &[u8],
        ) -> Result<DispatchOutcome, EndpointDriverError> {
            Ok(DispatchOutcome::Idle)
        }

        fn poll(
            &mut self,
            _budget: usize,
        ) -> Result<Vec<OwnedEndpointPacket>, EndpointDriverError> {
            Ok(Vec::new())
        }

        fn close(&mut self, _deadline_micros: u64) -> Result<(), EndpointDriverError> {
            let mut state = self.state.borrow_mut();
            state.closes += 1;
            if state.close_fails {
                Err(EndpointDriverError::Failed)
            } else {
                Ok(())
            }
        }
    }

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn descriptor() -> EndpointDescriptor {
        EndpointDescriptor {
            engine: handle(1),
            role: Role::Logic,
            actor: handle(2),
            artifact_digest: [1; 32],
            schema_fingerprint: [2; 32],
        }
    }

    #[test]
    fn wake_poll_close_and_generation_reuse_are_fail_closed() {
        let mut arena = EndpointArena::new(EndpointArenaConfig::default()).unwrap();
        let state = Rc::new(RefCell::new(DriverState::default()));
        let endpoint = arena
            .create(
                descriptor(),
                Box::new(Driver {
                    state: state.clone(),
                }),
            )
            .unwrap();
        assert_eq!(
            arena.poll_after_wake(endpoint, handle(2), 1, false),
            Err(EndpointError::WakeRequired)
        );
        arena.signal_wake(endpoint, handle(2)).unwrap();
        assert!(arena
            .poll_after_wake(endpoint, handle(2), 1, false)
            .unwrap()
            .is_empty());

        state.borrow_mut().close_fails = true;
        assert_eq!(
            arena.close(endpoint, handle(2), 10),
            Err(EndpointError::Driver(EndpointDriverError::Failed))
        );
        assert_eq!(arena.state(endpoint), Ok(EndpointState::Running));
        state.borrow_mut().close_fails = false;
        arena.close(endpoint, handle(2), 10).unwrap();
        arena.destroy(endpoint, handle(2)).unwrap();

        let replacement = arena
            .create(
                descriptor(),
                Box::new(Driver {
                    state: Rc::new(RefCell::new(DriverState::default())),
                }),
            )
            .unwrap();
        assert_eq!(replacement.handle.index, endpoint.handle.index);
        assert!(replacement.handle.generation > endpoint.handle.generation);
        assert_eq!(arena.state(endpoint), Err(EndpointError::StaleHandle));
    }

    #[test]
    fn descriptor_validation_rejects_zero_identity_before_slot_allocation() {
        let mut arena = EndpointArena::new(EndpointArenaConfig::default()).unwrap();
        let mut invalid = descriptor();
        invalid.artifact_digest = [0; 32];
        assert_eq!(
            arena.create(
                invalid,
                Box::new(Driver {
                    state: Rc::new(RefCell::new(DriverState::default())),
                }),
            ),
            Err(EndpointError::InvalidDescriptor)
        );
    }
}
