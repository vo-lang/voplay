use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::input::InputScopeId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HapticsConfig {
    pub max_devices: usize,
    pub max_pending: usize,
    pub max_commands: usize,
    pub max_completions: usize,
    pub max_duration_millis: u32,
}

impl Default for HapticsConfig {
    fn default() -> Self {
        Self {
            max_devices: 64,
            max_pending: 256,
            max_commands: 512,
            max_completions: 512,
            max_duration_millis: 60_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RumbleRequest {
    pub request_id: u64,
    pub scope: InputScopeId,
    pub device: Handle,
    pub duration_millis: u32,
    pub strong_magnitude: u16,
    pub weak_magnitude: u16,
    pub deadline_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HapticsCommand {
    Start(RumbleRequest),
    Cancel { request_id: u64, device: Handle },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HapticsOutcome {
    Succeeded,
    Unsupported,
    Cancelled,
    DeviceLost,
    Failed,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HapticsCompletion {
    pub request_id: u64,
    pub scope: InputScopeId,
    pub device: Handle,
    pub outcome: HapticsOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HapticsError {
    InvalidConfig,
    WrongEngine,
    InvalidIdentity,
    UnknownDevice,
    DeviceCapacity,
    DuplicateRequest,
    UnknownRequest,
    RequestCapacity,
    CommandCapacity,
    CompletionCapacity,
    InvalidDuration,
    InvalidMagnitude,
    InvalidDeadline,
    StaleDevice,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HapticsOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub devices: usize,
    pub pending_requests: usize,
    pub queued_commands: usize,
    pub queued_completions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HapticsShutdownReport {
    pub before: HapticsOwnerSnapshot,
    pub cancel_commands: Vec<HapticsCommand>,
    pub terminal_completions: Vec<HapticsCompletion>,
    pub after: HapticsOwnerSnapshot,
}

pub struct HapticsBridge {
    engine: EngineId,
    config: HapticsConfig,
    devices: BTreeSet<Handle>,
    pending: BTreeMap<u64, RumbleRequest>,
    commands: VecDeque<HapticsCommand>,
    completions: VecDeque<HapticsCompletion>,
    closed: bool,
}

impl HapticsBridge {
    pub fn new(engine: EngineId, config: HapticsConfig) -> Result<Self, HapticsError> {
        if !engine.is_valid()
            || config.max_devices == 0
            || config.max_pending == 0
            || config.max_commands == 0
            || config.max_completions == 0
            || config.max_duration_millis == 0
        {
            return Err(HapticsError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            devices: BTreeSet::new(),
            pending: BTreeMap::new(),
            commands: VecDeque::new(),
            completions: VecDeque::new(),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> HapticsOwnerSnapshot {
        HapticsOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            devices: self.devices.len(),
            pending_requests: self.pending.len(),
            queued_commands: self.commands.len(),
            queued_completions: self.completions.len(),
        }
    }

    pub fn connect_device(&mut self, device: Handle) -> Result<(), HapticsError> {
        self.ensure_open()?;
        if !device.is_valid() {
            return Err(HapticsError::InvalidIdentity);
        }
        if self.devices.contains(&device) {
            return Ok(());
        }
        if self.devices.len() == self.config.max_devices {
            return Err(HapticsError::DeviceCapacity);
        }
        if self
            .devices
            .iter()
            .any(|existing| existing.index == device.index)
        {
            return Err(HapticsError::StaleDevice);
        }
        self.devices.insert(device);
        Ok(())
    }

    pub fn disconnect_device(&mut self, device: Handle) -> Result<usize, HapticsError> {
        self.ensure_open()?;
        if !self.devices.contains(&device) {
            return Err(HapticsError::UnknownDevice);
        }
        let requests = self
            .pending
            .values()
            .filter(|request| request.device == device)
            .copied()
            .collect::<Vec<_>>();
        let terminal_count = requests.len();
        self.preflight_terminal(terminal_count)?;
        let request_ids = requests
            .iter()
            .map(|request| request.request_id)
            .collect::<BTreeSet<_>>();
        let remaining_commands = self
            .commands
            .iter()
            .filter(|command| {
                !matches!(
                    command,
                    HapticsCommand::Start(request)
                        if request_ids.contains(&request.request_id)
                )
            })
            .count();
        if remaining_commands
            .checked_add(requests.len())
            .is_none_or(|count| count > self.config.max_commands)
        {
            return Err(HapticsError::CommandCapacity);
        }
        self.commands.retain(|command| {
            !matches!(
                command,
                HapticsCommand::Start(request) if request_ids.contains(&request.request_id)
            )
        });
        self.devices.remove(&device);
        for request in requests {
            self.pending.remove(&request.request_id);
            self.commands.push_back(HapticsCommand::Cancel {
                request_id: request.request_id,
                device,
            });
            self.completions.push_back(HapticsCompletion {
                request_id: request.request_id,
                scope: request.scope,
                device,
                outcome: HapticsOutcome::DeviceLost,
            });
        }
        Ok(terminal_count)
    }

    pub fn request(&mut self, request: RumbleRequest) -> Result<(), HapticsError> {
        self.ensure_open()?;
        self.validate_request(request)?;
        if self.pending.contains_key(&request.request_id) {
            return Err(HapticsError::DuplicateRequest);
        }
        if self.pending.len() == self.config.max_pending {
            return Err(HapticsError::RequestCapacity);
        }
        if self
            .completions
            .len()
            .checked_add(self.pending.len())
            .and_then(|count| count.checked_add(1))
            .is_none_or(|count| count > self.config.max_completions)
        {
            return Err(HapticsError::CompletionCapacity);
        }
        if self.commands.len() == self.config.max_commands {
            return Err(HapticsError::CommandCapacity);
        }
        self.pending.insert(request.request_id, request);
        self.commands.push_back(HapticsCommand::Start(request));
        Ok(())
    }

    pub fn cancel(&mut self, request_id: u64) -> Result<(), HapticsError> {
        self.ensure_open()?;
        let request = self
            .pending
            .get(&request_id)
            .copied()
            .ok_or(HapticsError::UnknownRequest)?;
        self.preflight_terminal(1)?;
        if self.commands.len() == self.config.max_commands {
            return Err(HapticsError::CommandCapacity);
        }
        self.pending.remove(&request_id);
        self.commands.push_back(HapticsCommand::Cancel {
            request_id,
            device: request.device,
        });
        self.completions.push_back(HapticsCompletion {
            request_id,
            scope: request.scope,
            device: request.device,
            outcome: HapticsOutcome::Cancelled,
        });
        Ok(())
    }

    pub fn complete(
        &mut self,
        request_id: u64,
        device: Handle,
        outcome: HapticsOutcome,
    ) -> Result<(), HapticsError> {
        self.ensure_open()?;
        if outcome == HapticsOutcome::TimedOut {
            return Err(HapticsError::InvalidIdentity);
        }
        let request = self
            .pending
            .get(&request_id)
            .copied()
            .ok_or(HapticsError::UnknownRequest)?;
        if request.device != device || !self.devices.contains(&device) {
            return Err(HapticsError::StaleDevice);
        }
        self.preflight_terminal(1)?;
        self.pending.remove(&request_id);
        self.completions.push_back(HapticsCompletion {
            request_id,
            scope: request.scope,
            device,
            outcome,
        });
        Ok(())
    }

    pub fn expire(&mut self, now_millis: u64) -> Result<usize, HapticsError> {
        self.ensure_open()?;
        let expired = self
            .pending
            .values()
            .filter(|request| request.deadline_millis <= now_millis)
            .copied()
            .collect::<Vec<_>>();
        self.preflight_terminal(expired.len())?;
        if self
            .commands
            .len()
            .checked_add(expired.len())
            .is_none_or(|count| count > self.config.max_commands)
        {
            return Err(HapticsError::CommandCapacity);
        }
        for request in &expired {
            self.pending.remove(&request.request_id);
            self.commands.push_back(HapticsCommand::Cancel {
                request_id: request.request_id,
                device: request.device,
            });
            self.completions.push_back(HapticsCompletion {
                request_id: request.request_id,
                scope: request.scope,
                device: request.device,
                outcome: HapticsOutcome::TimedOut,
            });
        }
        Ok(expired.len())
    }

    pub fn poll_command(&mut self) -> Option<HapticsCommand> {
        if self.closed {
            return None;
        }
        self.commands.pop_front()
    }

    pub fn next_command(&self) -> Option<HapticsCommand> {
        if self.closed {
            return None;
        }
        self.commands.front().copied()
    }

    pub fn consume_command(&mut self) -> Option<HapticsCommand> {
        if self.closed {
            return None;
        }
        self.commands.pop_front()
    }

    pub fn poll_completion(&mut self) -> Option<HapticsCompletion> {
        if self.closed {
            return None;
        }
        self.completions.pop_front()
    }

    pub fn shutdown(&mut self) -> HapticsShutdownReport {
        let before = self.owner_snapshot();
        if self.closed {
            return HapticsShutdownReport {
                before,
                cancel_commands: Vec::new(),
                terminal_completions: Vec::new(),
                after: before,
            };
        }
        let cancel_commands = self
            .pending
            .values()
            .map(|request| HapticsCommand::Cancel {
                request_id: request.request_id,
                device: request.device,
            })
            .collect::<Vec<_>>();
        let mut terminal_completions = self.completions.drain(..).collect::<Vec<_>>();
        terminal_completions.extend(self.pending.values().map(|request| HapticsCompletion {
            request_id: request.request_id,
            scope: request.scope,
            device: request.device,
            outcome: HapticsOutcome::Cancelled,
        }));
        self.devices.clear();
        self.pending.clear();
        self.commands.clear();
        self.closed = true;
        HapticsShutdownReport {
            before,
            cancel_commands,
            terminal_completions,
            after: self.owner_snapshot(),
        }
    }

    fn validate_request(&self, request: RumbleRequest) -> Result<(), HapticsError> {
        if request.request_id == 0
            || request.scope.engine != self.engine
            || !request.scope.handle.is_valid()
            || !request.device.is_valid()
        {
            return Err(HapticsError::InvalidIdentity);
        }
        if !self.devices.contains(&request.device) {
            return Err(HapticsError::UnknownDevice);
        }
        if request.duration_millis == 0 || request.duration_millis > self.config.max_duration_millis
        {
            return Err(HapticsError::InvalidDuration);
        }
        if request.strong_magnitude == 0 && request.weak_magnitude == 0 {
            return Err(HapticsError::InvalidMagnitude);
        }
        if request.deadline_millis == 0 {
            return Err(HapticsError::InvalidDeadline);
        }
        Ok(())
    }

    fn preflight_terminal(&self, count: usize) -> Result<(), HapticsError> {
        if self
            .completions
            .len()
            .checked_add(count)
            .is_none_or(|next| next > self.config.max_completions)
        {
            return Err(HapticsError::CompletionCapacity);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), HapticsError> {
        if self.closed {
            Err(HapticsError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn request(id: u64, device: Handle) -> RumbleRequest {
        RumbleRequest {
            request_id: id,
            scope: InputScopeId {
                engine: handle(1, 1),
                handle: handle(2, 1),
            },
            device,
            duration_millis: 50,
            strong_magnitude: 100,
            weak_magnitude: 0,
            deadline_millis: 100,
        }
    }

    #[test]
    fn command_capacity_failure_preserves_pending_request_for_later_cancel() {
        let config = HapticsConfig {
            max_commands: 1,
            ..HapticsConfig::default()
        };
        let device = handle(3, 1);
        let mut bridge = HapticsBridge::new(handle(1, 1), config).unwrap();
        bridge.connect_device(device).unwrap();
        bridge.request(request(1, device)).unwrap();
        assert_eq!(bridge.cancel(1), Err(HapticsError::CommandCapacity));
        assert!(matches!(
            bridge.poll_command(),
            Some(HapticsCommand::Start(RumbleRequest { request_id: 1, .. }))
        ));
        bridge.cancel(1).unwrap();
        assert!(matches!(
            bridge.poll_command(),
            Some(HapticsCommand::Cancel { request_id: 1, .. })
        ));
        assert_eq!(
            bridge.poll_completion().unwrap().outcome,
            HapticsOutcome::Cancelled
        );
    }

    #[test]
    fn device_disconnect_completes_live_requests_and_enforces_device_generation() {
        let device = handle(3, 1);
        let mut bridge = HapticsBridge::new(handle(1, 1), HapticsConfig::default()).unwrap();
        bridge.connect_device(device).unwrap();
        bridge.request(request(1, device)).unwrap();
        assert_eq!(
            bridge.connect_device(handle(3, 2)),
            Err(HapticsError::StaleDevice)
        );
        assert_eq!(bridge.disconnect_device(device), Ok(1));
        assert_eq!(
            bridge.poll_completion().unwrap(),
            HapticsCompletion {
                request_id: 1,
                scope: request(1, device).scope,
                device,
                outcome: HapticsOutcome::DeviceLost,
            }
        );
        bridge.connect_device(handle(3, 2)).unwrap();
    }
}
