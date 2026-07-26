use std::{cell::Cell, collections::BTreeMap};

use voplay_protocol::{EngineId, Handle};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DeviceHubId(pub Handle);

impl DeviceHubId {
    pub const fn is_valid(self) -> bool {
        self.0.is_valid()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DeviceRef {
    pub hub: DeviceHubId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EngineDeviceLease {
    pub hub: DeviceHubId,
    pub handle: Handle,
    pub engine: EngineId,
    pub device: DeviceRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceHubConfig {
    pub max_devices: usize,
    pub max_engine_bindings: usize,
}

impl Default for DeviceHubConfig {
    fn default() -> Self {
        Self {
            max_devices: 8,
            max_engine_bindings: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceState {
    Ready,
    Lost,
    Recovering,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineDeviceState {
    Ready,
    RendererRecoveryRequired,
    DeviceLost,
    AwaitingDeviceRebind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceLossReason {
    Removed,
    Reset,
    DriverFault,
    OutOfMemory,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineDeviceStatus {
    pub state: EngineDeviceState,
    pub device_generation: u64,
    pub render_endpoint: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceStatus {
    pub state: DeviceState,
    pub generation: u64,
    pub last_loss: Option<DeviceLossReason>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceHubMetrics {
    pub devices: usize,
    pub peak_devices: usize,
    pub engine_bindings: usize,
    pub peak_engine_bindings: usize,
    pub attached_engines: u64,
    pub detached_engines: u64,
    pub renderer_faults: u64,
    pub renderer_recoveries: u64,
    pub device_losses: u64,
    pub device_recoveries: u64,
    pub stale_rejections: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceHubError {
    Closed,
    InvalidConfig,
    WrongHub,
    InvalidDevice,
    InvalidEngine,
    DeviceCapacity,
    BindingCapacity,
    EngineAlreadyAttached,
    StaleLease,
    StaleDeviceGeneration,
    StaleRenderEndpoint,
    InvalidState,
    GenerationExhausted,
}

struct DeviceSlot {
    handle: Handle,
    generation: u64,
    state: DeviceState,
    last_loss: Option<DeviceLossReason>,
}

struct BindingSlot {
    lease: EngineDeviceLease,
    state: EngineDeviceState,
    device_generation: u64,
    render_endpoint: Handle,
}

pub struct DeviceHub {
    id: DeviceHubId,
    config: DeviceHubConfig,
    next_device_index: u32,
    next_lease_index: u32,
    devices: BTreeMap<u32, DeviceSlot>,
    bindings: BTreeMap<u32, BindingSlot>,
    engine_bindings: BTreeMap<EngineId, u32>,
    metrics: Cell<DeviceHubMetrics>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceHubOwnerSnapshot {
    pub closed: bool,
    pub devices: usize,
    pub engine_bindings: usize,
    pub ready_devices: usize,
    pub lost_devices: usize,
    pub recovering_devices: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceHubShutdownReport {
    pub released_devices: usize,
    pub released_engine_leases: Vec<EngineDeviceLease>,
}

impl DeviceHub {
    pub fn new(id: DeviceHubId, config: DeviceHubConfig) -> Result<Self, DeviceHubError> {
        if !id.is_valid() || config.max_devices == 0 || config.max_engine_bindings == 0 {
            return Err(DeviceHubError::InvalidConfig);
        }
        Ok(Self {
            id,
            config,
            next_device_index: 0,
            next_lease_index: 0,
            devices: BTreeMap::new(),
            bindings: BTreeMap::new(),
            engine_bindings: BTreeMap::new(),
            metrics: Cell::new(DeviceHubMetrics::default()),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> DeviceHubOwnerSnapshot {
        DeviceHubOwnerSnapshot {
            closed: self.closed,
            devices: self.devices.len(),
            engine_bindings: self.bindings.len(),
            ready_devices: self
                .devices
                .values()
                .filter(|device| device.state == DeviceState::Ready)
                .count(),
            lost_devices: self
                .devices
                .values()
                .filter(|device| device.state == DeviceState::Lost)
                .count(),
            recovering_devices: self
                .devices
                .values()
                .filter(|device| device.state == DeviceState::Recovering)
                .count(),
        }
    }

    pub fn shutdown(&mut self) -> DeviceHubShutdownReport {
        if self.closed {
            return DeviceHubShutdownReport {
                released_devices: 0,
                released_engine_leases: Vec::new(),
            };
        }
        let released_engine_leases = self
            .bindings
            .values()
            .map(|binding| binding.lease)
            .collect();
        let released_devices = self.devices.len();
        self.closed = true;
        self.bindings.clear();
        self.engine_bindings.clear();
        self.devices.clear();
        let mut metrics = self.metrics.get();
        metrics.detached_engines = metrics
            .detached_engines
            .saturating_add(metrics.engine_bindings as u64);
        metrics.engine_bindings = 0;
        metrics.devices = 0;
        self.metrics.set(metrics);
        DeviceHubShutdownReport {
            released_devices,
            released_engine_leases,
        }
    }

    pub fn register_device(&mut self) -> Result<DeviceRef, DeviceHubError> {
        self.ensure_open()?;
        if self.devices.len() == self.config.max_devices {
            return Err(DeviceHubError::DeviceCapacity);
        }
        let index = self.next_device_index;
        self.next_device_index = self
            .next_device_index
            .checked_add(1)
            .ok_or(DeviceHubError::GenerationExhausted)?;
        let handle = Handle {
            index,
            generation: 1,
        };
        self.devices.insert(
            index,
            DeviceSlot {
                handle,
                generation: 1,
                state: DeviceState::Ready,
                last_loss: None,
            },
        );
        let mut metrics = self.metrics.get();
        metrics.devices = self.devices.len();
        metrics.peak_devices = metrics.peak_devices.max(metrics.devices);
        self.metrics.set(metrics);
        Ok(DeviceRef {
            hub: self.id,
            handle,
        })
    }

    pub fn attach_engine(
        &mut self,
        engine: EngineId,
        device: DeviceRef,
        render_endpoint: Handle,
    ) -> Result<EngineDeviceLease, DeviceHubError> {
        self.ensure_open()?;
        if !engine.is_valid() || !render_endpoint.is_valid() {
            return Err(DeviceHubError::InvalidEngine);
        }
        let device_generation = {
            let slot = self.device(device)?;
            if slot.state != DeviceState::Ready {
                return Err(DeviceHubError::InvalidState);
            }
            slot.generation
        };
        if self.engine_bindings.contains_key(&engine) {
            return Err(DeviceHubError::EngineAlreadyAttached);
        }
        if self.bindings.len() == self.config.max_engine_bindings {
            return Err(DeviceHubError::BindingCapacity);
        }
        let index = self.next_lease_index;
        self.next_lease_index = self
            .next_lease_index
            .checked_add(1)
            .ok_or(DeviceHubError::GenerationExhausted)?;
        let lease = EngineDeviceLease {
            hub: self.id,
            handle: Handle {
                index,
                generation: 1,
            },
            engine,
            device,
        };
        self.bindings.insert(
            index,
            BindingSlot {
                lease,
                state: EngineDeviceState::Ready,
                device_generation,
                render_endpoint,
            },
        );
        self.engine_bindings.insert(engine, index);
        let mut metrics = self.metrics.get();
        metrics.engine_bindings = self.bindings.len();
        metrics.peak_engine_bindings = metrics.peak_engine_bindings.max(metrics.engine_bindings);
        metrics.attached_engines = metrics.attached_engines.saturating_add(1);
        self.metrics.set(metrics);
        Ok(lease)
    }

    pub fn status(&self, lease: EngineDeviceLease) -> Result<EngineDeviceStatus, DeviceHubError> {
        self.ensure_open()?;
        let binding = self.binding(lease)?;
        Ok(EngineDeviceStatus {
            state: binding.state,
            device_generation: binding.device_generation,
            render_endpoint: binding.render_endpoint,
        })
    }

    pub fn device_status(&self, device: DeviceRef) -> Result<DeviceStatus, DeviceHubError> {
        self.ensure_open()?;
        let device = self.device(device)?;
        Ok(DeviceStatus {
            state: device.state,
            generation: device.generation,
            last_loss: device.last_loss,
        })
    }

    pub fn lease_for_engine(&self, engine: EngineId) -> Option<EngineDeviceLease> {
        self.engine_bindings
            .get(&engine)
            .and_then(|index| self.bindings.get(index))
            .map(|binding| binding.lease)
    }

    pub fn leases_for_device(
        &self,
        device: DeviceRef,
    ) -> Result<Vec<EngineDeviceLease>, DeviceHubError> {
        self.ensure_open()?;
        self.device(device)?;
        Ok(self
            .bindings
            .values()
            .filter(|binding| binding.lease.device == device)
            .map(|binding| binding.lease)
            .collect())
    }

    pub fn report_renderer_fault(
        &mut self,
        lease: EngineDeviceLease,
        endpoint: Handle,
    ) -> Result<Handle, DeviceHubError> {
        self.ensure_open()?;
        let binding = self.binding_mut(lease)?;
        if binding.state != EngineDeviceState::Ready {
            return Err(DeviceHubError::InvalidState);
        }
        if binding.render_endpoint != endpoint {
            return Err(DeviceHubError::StaleRenderEndpoint);
        }
        let generation = endpoint
            .generation
            .checked_add(1)
            .ok_or(DeviceHubError::GenerationExhausted)?;
        binding.state = EngineDeviceState::RendererRecoveryRequired;
        binding.render_endpoint = Handle {
            index: endpoint.index,
            generation,
        };
        let replacement = binding.render_endpoint;
        let mut metrics = self.metrics.get();
        metrics.renderer_faults = metrics.renderer_faults.saturating_add(1);
        self.metrics.set(metrics);
        Ok(replacement)
    }

    pub fn complete_renderer_rebind(
        &mut self,
        lease: EngineDeviceLease,
        device_generation: u64,
        endpoint: Handle,
    ) -> Result<(), DeviceHubError> {
        self.ensure_open()?;
        let binding = self.binding_mut(lease)?;
        if binding.state != EngineDeviceState::RendererRecoveryRequired {
            return Err(DeviceHubError::InvalidState);
        }
        if binding.device_generation != device_generation {
            return Err(DeviceHubError::StaleDeviceGeneration);
        }
        if binding.render_endpoint != endpoint {
            return Err(DeviceHubError::StaleRenderEndpoint);
        }
        binding.state = EngineDeviceState::Ready;
        let mut metrics = self.metrics.get();
        metrics.renderer_recoveries = metrics.renderer_recoveries.saturating_add(1);
        self.metrics.set(metrics);
        Ok(())
    }

    pub fn report_device_lost(
        &mut self,
        device: DeviceRef,
        generation: u64,
        reason: DeviceLossReason,
    ) -> Result<(), DeviceHubError> {
        self.ensure_open()?;
        let slot = self.device_mut(device)?;
        if slot.state != DeviceState::Ready || slot.generation != generation {
            return Err(DeviceHubError::StaleDeviceGeneration);
        }
        slot.state = DeviceState::Lost;
        slot.last_loss = Some(reason);
        for binding in self
            .bindings
            .values_mut()
            .filter(|binding| binding.lease.device == device)
        {
            binding.state = EngineDeviceState::DeviceLost;
        }
        let mut metrics = self.metrics.get();
        metrics.device_losses = metrics.device_losses.saturating_add(1);
        self.metrics.set(metrics);
        Ok(())
    }

    pub fn begin_device_recovery(&mut self, device: DeviceRef) -> Result<u64, DeviceHubError> {
        self.ensure_open()?;
        let generation = {
            let slot = self.device(device)?;
            if slot.state != DeviceState::Lost {
                return Err(DeviceHubError::InvalidState);
            }
            slot.generation
                .checked_add(1)
                .ok_or(DeviceHubError::GenerationExhausted)?
        };
        let binding_indexes = self
            .bindings
            .iter()
            .filter(|(_, binding)| binding.lease.device == device)
            .map(|(index, binding)| {
                binding
                    .render_endpoint
                    .generation
                    .checked_add(1)
                    .map(|replacement_generation| (*index, replacement_generation))
                    .ok_or(DeviceHubError::GenerationExhausted)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let slot = self.device_mut(device)?;
        slot.generation = generation;
        slot.state = if binding_indexes.is_empty() {
            DeviceState::Ready
        } else {
            DeviceState::Recovering
        };
        for (index, replacement_generation) in &binding_indexes {
            let binding = self.bindings.get_mut(index).unwrap();
            binding.state = EngineDeviceState::AwaitingDeviceRebind;
            binding.device_generation = generation;
            binding.render_endpoint.generation = *replacement_generation;
        }
        let mut metrics = self.metrics.get();
        metrics.device_recoveries = metrics.device_recoveries.saturating_add(1);
        self.metrics.set(metrics);
        Ok(generation)
    }

    pub fn complete_device_rebind(
        &mut self,
        lease: EngineDeviceLease,
        device_generation: u64,
        endpoint: Handle,
    ) -> Result<(), DeviceHubError> {
        self.ensure_open()?;
        let device = lease.device;
        {
            let binding = self.binding_mut(lease)?;
            if binding.state != EngineDeviceState::AwaitingDeviceRebind {
                return Err(DeviceHubError::InvalidState);
            }
            if binding.device_generation != device_generation {
                return Err(DeviceHubError::StaleDeviceGeneration);
            }
            if binding.render_endpoint != endpoint {
                return Err(DeviceHubError::StaleRenderEndpoint);
            }
            binding.state = EngineDeviceState::Ready;
        }
        if self
            .bindings
            .values()
            .filter(|binding| binding.lease.device == device)
            .all(|binding| binding.state == EngineDeviceState::Ready)
        {
            self.device_mut(device)?.state = DeviceState::Ready;
        }
        Ok(())
    }

    pub fn detach_engine(&mut self, lease: EngineDeviceLease) -> Result<(), DeviceHubError> {
        self.ensure_open()?;
        self.binding(lease)?;
        let device = lease.device;
        self.bindings.remove(&lease.handle.index);
        self.engine_bindings.remove(&lease.engine);
        if self
            .devices
            .get(&device.handle.index)
            .is_some_and(|slot| slot.state == DeviceState::Recovering)
            && self
                .bindings
                .values()
                .filter(|binding| binding.lease.device == device)
                .all(|binding| binding.state == EngineDeviceState::Ready)
        {
            self.devices
                .get_mut(&device.handle.index)
                .expect("validated bound device remains registered")
                .state = DeviceState::Ready;
        }
        let mut metrics = self.metrics.get();
        metrics.engine_bindings = self.bindings.len();
        metrics.detached_engines = metrics.detached_engines.saturating_add(1);
        self.metrics.set(metrics);
        Ok(())
    }

    pub fn metrics(&self) -> DeviceHubMetrics {
        self.metrics.get()
    }

    pub fn owner_statuses(&self) -> Vec<(EngineDeviceLease, EngineDeviceStatus)> {
        self.bindings
            .values()
            .map(|binding| {
                (
                    binding.lease,
                    EngineDeviceStatus {
                        state: binding.state,
                        device_generation: binding.device_generation,
                        render_endpoint: binding.render_endpoint,
                    },
                )
            })
            .collect()
    }

    fn record_stale_rejection(&self) {
        let mut metrics = self.metrics.get();
        metrics.stale_rejections = metrics.stale_rejections.saturating_add(1);
        self.metrics.set(metrics);
    }

    fn device(&self, device: DeviceRef) -> Result<&DeviceSlot, DeviceHubError> {
        if device.hub != self.id {
            self.record_stale_rejection();
            return Err(DeviceHubError::WrongHub);
        }
        let result = self
            .devices
            .get(&device.handle.index)
            .filter(|slot| slot.handle == device.handle)
            .ok_or(DeviceHubError::InvalidDevice);
        if result.is_err() {
            self.record_stale_rejection();
        }
        result
    }

    fn device_mut(&mut self, device: DeviceRef) -> Result<&mut DeviceSlot, DeviceHubError> {
        if device.hub != self.id {
            self.record_stale_rejection();
            return Err(DeviceHubError::WrongHub);
        }
        if self
            .devices
            .get(&device.handle.index)
            .is_none_or(|slot| slot.handle != device.handle)
        {
            self.record_stale_rejection();
            return Err(DeviceHubError::InvalidDevice);
        }
        self.devices
            .get_mut(&device.handle.index)
            .ok_or(DeviceHubError::InvalidDevice)
    }

    fn binding(&self, lease: EngineDeviceLease) -> Result<&BindingSlot, DeviceHubError> {
        if lease.hub != self.id {
            self.record_stale_rejection();
            return Err(DeviceHubError::WrongHub);
        }
        let result = self
            .bindings
            .get(&lease.handle.index)
            .filter(|binding| binding.lease == lease)
            .ok_or(DeviceHubError::StaleLease);
        if result.is_err() {
            self.record_stale_rejection();
        }
        result
    }

    fn binding_mut(
        &mut self,
        lease: EngineDeviceLease,
    ) -> Result<&mut BindingSlot, DeviceHubError> {
        if lease.hub != self.id {
            self.record_stale_rejection();
            return Err(DeviceHubError::WrongHub);
        }
        if self
            .bindings
            .get(&lease.handle.index)
            .is_none_or(|binding| binding.lease != lease)
        {
            self.record_stale_rejection();
            return Err(DeviceHubError::StaleLease);
        }
        self.bindings
            .get_mut(&lease.handle.index)
            .ok_or(DeviceHubError::StaleLease)
    }

    fn ensure_open(&self) -> Result<(), DeviceHubError> {
        if self.closed {
            Err(DeviceHubError::Closed)
        } else {
            Ok(())
        }
    }
}
