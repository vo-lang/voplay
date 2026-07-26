use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::{EngineId, Handle};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InputScopeId {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedInputEvent {
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub scope: InputScopeId,
    pub device: Handle,
    pub kind: u16,
    pub code: u32,
    pub value: i32,
    pub modifiers: u16,
    pub consumed_by_ui: bool,
    pub payload: Vec<u8>,
}

impl NormalizedInputEvent {
    pub fn wire_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(74 + self.payload.len());
        bytes.extend_from_slice(b"voplay-input-v1\0");
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&self.timestamp_micros.to_le_bytes());
        push_handle(&mut bytes, self.scope.engine);
        push_handle(&mut bytes, self.scope.handle);
        push_handle(&mut bytes, self.device);
        bytes.extend_from_slice(&self.kind.to_le_bytes());
        bytes.extend_from_slice(&self.code.to_le_bytes());
        bytes.extend_from_slice(&self.value.to_le_bytes());
        bytes.extend_from_slice(&self.modifiers.to_le_bytes());
        bytes.push(u8::from(self.consumed_by_ui));
        bytes.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionBinding {
    pub kind: u16,
    pub code: u32,
    pub action: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionSample {
    pub action: u32,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDeviceKind {
    Keyboard,
    Pointer,
    Touch,
    Gamepad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceDescriptor {
    pub device: Handle,
    pub kind: InputDeviceKind,
    pub platform_index: u32,
    pub mapping: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionMapSnapshot {
    pub revision: u64,
    pub bindings: Vec<ActionBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformInputAdmission {
    Connected,
    Disconnected { released: usize },
    FocusChanged { focused: bool, released: usize },
    Queued(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputDomainShutdownReport {
    pub released_scopes: usize,
    pub released_events: usize,
    pub released_bytes: usize,
    pub released_devices: usize,
    pub released_bindings: usize,
}

pub const PLATFORM_INPUT_FOCUS: u16 = 12;
pub const PLATFORM_INPUT_GAMEPAD_CONNECT: u16 = 13;
pub const PLATFORM_INPUT_GAMEPAD_DISCONNECT: u16 = 14;
pub const PLATFORM_INPUT_GAMEPAD_BUTTON: u16 = 15;
pub const PLATFORM_INPUT_GAMEPAD_AXIS: u16 = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TickInputFrame {
    pub tick_id: u64,
    pub scope: InputScopeId,
    pub from_timestamp_micros: u64,
    pub through_timestamp_micros: u64,
    pub through_sequence: u64,
    pub action_map_revision: u64,
    pub events: Vec<NormalizedInputEvent>,
    pub actions: Vec<ActionSample>,
}

impl TickInputFrame {
    pub fn deterministic_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.tick_id.to_le_bytes());
        push_handle(&mut bytes, self.scope.engine);
        push_handle(&mut bytes, self.scope.handle);
        bytes.extend_from_slice(&self.from_timestamp_micros.to_le_bytes());
        bytes.extend_from_slice(&self.through_timestamp_micros.to_le_bytes());
        bytes.extend_from_slice(&self.through_sequence.to_le_bytes());
        bytes.extend_from_slice(&self.action_map_revision.to_le_bytes());
        bytes.extend_from_slice(&(self.events.len() as u32).to_le_bytes());
        for event in &self.events {
            bytes.extend_from_slice(&event.sequence.to_le_bytes());
            bytes.extend_from_slice(&event.timestamp_micros.to_le_bytes());
            push_handle(&mut bytes, event.device);
            bytes.extend_from_slice(&event.kind.to_le_bytes());
            bytes.extend_from_slice(&event.code.to_le_bytes());
            bytes.extend_from_slice(&event.value.to_le_bytes());
            bytes.extend_from_slice(&event.modifiers.to_le_bytes());
            bytes.extend_from_slice(&(event.payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&event.payload);
        }
        bytes.extend_from_slice(&(self.actions.len() as u32).to_le_bytes());
        for action in &self.actions {
            bytes.extend_from_slice(&action.action.to_le_bytes());
            bytes.extend_from_slice(&action.value.to_le_bytes());
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputDomainConfig {
    pub max_scopes: usize,
    pub max_queued_events: usize,
    pub max_queued_bytes: usize,
    pub max_event_bytes: usize,
    pub max_bindings: usize,
}

impl Default for InputDomainConfig {
    fn default() -> Self {
        Self {
            max_scopes: 64,
            max_queued_events: 4096,
            max_queued_bytes: 4 * 1024 * 1024,
            max_event_bytes: 64 * 1024,
            max_bindings: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputError {
    InvalidConfig,
    WrongEngine,
    InvalidScope,
    InvalidDevice,
    Sequence,
    EventCapacity,
    EventByteCapacity,
    ScopeCapacity,
    UnknownScope,
    BindingCapacity,
    InvalidBinding,
    DuplicateBinding,
    TickSequence,
    DeviceCapacity,
    DuplicateDevice,
    UnknownDevice,
    StaleDevice,
    ActionMapRevision,
    TimestampBoundary,
    SequenceExhausted,
    Closed,
}

pub struct InputDomain {
    engine: EngineId,
    config: InputDomainConfig,
    last_sequence: u64,
    queued_events: usize,
    queued_bytes: usize,
    scopes: BTreeSet<InputScopeId>,
    queues: BTreeMap<InputScopeId, VecDeque<NormalizedInputEvent>>,
    scope_sequences: BTreeMap<InputScopeId, u64>,
    last_ticks: BTreeMap<InputScopeId, u64>,
    bindings: BTreeMap<(u16, u32), u32>,
    action_map_revision: u64,
    action_state: BTreeMap<(InputScopeId, u32), i32>,
    devices: BTreeMap<Handle, InputDeviceDescriptor>,
    pressed: BTreeMap<(InputScopeId, Handle, u16, u32), i32>,
    focused_scopes: BTreeSet<InputScopeId>,
    last_tick_boundaries: BTreeMap<InputScopeId, u64>,
    last_event_timestamps: BTreeMap<InputScopeId, u64>,
    max_devices: usize,
    closed: bool,
}

impl InputDomain {
    pub fn scope_count(&self) -> usize {
        self.scopes.len()
    }

    pub fn shutdown(&mut self) -> InputDomainShutdownReport {
        if self.closed {
            return InputDomainShutdownReport {
                released_scopes: 0,
                released_events: 0,
                released_bytes: 0,
                released_devices: 0,
                released_bindings: 0,
            };
        }
        let report = InputDomainShutdownReport {
            released_scopes: self.scopes.len(),
            released_events: self.queued_events,
            released_bytes: self.queued_bytes,
            released_devices: self.devices.len(),
            released_bindings: self.bindings.len(),
        };
        self.scopes.clear();
        self.queues.clear();
        self.scope_sequences.clear();
        self.last_ticks.clear();
        self.bindings.clear();
        self.action_state.clear();
        self.devices.clear();
        self.pressed.clear();
        self.focused_scopes.clear();
        self.last_tick_boundaries.clear();
        self.last_event_timestamps.clear();
        self.queued_events = 0;
        self.queued_bytes = 0;
        self.closed = true;
        report
    }

    pub fn new(engine: EngineId, config: InputDomainConfig) -> Result<Self, InputError> {
        if !engine.is_valid()
            || config.max_scopes == 0
            || config.max_queued_events == 0
            || config.max_queued_bytes == 0
            || config.max_event_bytes == 0
            || config.max_bindings == 0
            || config.max_queued_events > u32::MAX as usize
            || config.max_event_bytes > u32::MAX as usize
            || config.max_bindings > u32::MAX as usize
        {
            return Err(InputError::InvalidConfig);
        }
        let max_devices = config
            .max_scopes
            .checked_mul(16)
            .ok_or(InputError::InvalidConfig)?;
        Ok(Self {
            engine,
            config,
            last_sequence: 0,
            queued_events: 0,
            queued_bytes: 0,
            scopes: BTreeSet::new(),
            queues: BTreeMap::new(),
            scope_sequences: BTreeMap::new(),
            last_ticks: BTreeMap::new(),
            bindings: BTreeMap::new(),
            action_map_revision: 0,
            action_state: BTreeMap::new(),
            devices: BTreeMap::new(),
            pressed: BTreeMap::new(),
            focused_scopes: BTreeSet::new(),
            last_tick_boundaries: BTreeMap::new(),
            last_event_timestamps: BTreeMap::new(),
            max_devices,
            closed: false,
        })
    }

    pub fn register_scope(&mut self, scope: InputScopeId) -> Result<(), InputError> {
        self.validate_scope_identity(scope)?;
        if self.scopes.contains(&scope) {
            return Ok(());
        }
        if self.scopes.len() == self.config.max_scopes {
            return Err(InputError::ScopeCapacity);
        }
        self.scopes.insert(scope);
        self.focused_scopes.insert(scope);
        Ok(())
    }

    pub fn unregister_scope(&mut self, scope: InputScopeId) -> Result<usize, InputError> {
        self.validate_scope(scope)?;
        self.scopes.remove(&scope);
        self.focused_scopes.remove(&scope);
        let queued = self.queues.remove(&scope).unwrap_or_default();
        self.queued_events -= queued.len();
        self.queued_bytes -= queued
            .iter()
            .map(|event| event.payload.len())
            .sum::<usize>();
        self.scope_sequences.remove(&scope);
        self.last_ticks.remove(&scope);
        self.last_tick_boundaries.remove(&scope);
        self.last_event_timestamps.remove(&scope);
        self.action_state
            .retain(|(action_scope, _), _| *action_scope != scope);
        self.pressed
            .retain(|(pressed_scope, _, _, _), _| *pressed_scope != scope);
        Ok(queued.len())
    }

    pub fn configure_bindings(&mut self, bindings: Vec<ActionBinding>) -> Result<(), InputError> {
        let revision = self
            .action_map_revision
            .checked_add(1)
            .ok_or(InputError::ActionMapRevision)?;
        self.configure_bindings_at(revision, bindings)
    }

    pub fn configure_bindings_at(
        &mut self,
        revision: u64,
        bindings: Vec<ActionBinding>,
    ) -> Result<(), InputError> {
        self.ensure_open()?;
        if revision == 0 || revision <= self.action_map_revision {
            return Err(InputError::ActionMapRevision);
        }
        if bindings.len() > self.config.max_bindings {
            return Err(InputError::BindingCapacity);
        }
        let mut next = BTreeMap::new();
        for binding in bindings {
            if binding.kind == 0 || binding.code == 0 || binding.action == 0 {
                return Err(InputError::InvalidBinding);
            }
            if next
                .insert((binding.kind, binding.code), binding.action)
                .is_some()
            {
                return Err(InputError::DuplicateBinding);
            }
        }
        self.bindings = next;
        self.action_map_revision = revision;
        self.action_state.clear();
        for (&(scope, _, kind, code), &value) in &self.pressed {
            if let Some(action) = self.bindings.get(&(kind, code)) {
                self.action_state.insert((scope, *action), value);
            }
        }
        Ok(())
    }

    pub fn action_map_snapshot(&self) -> ActionMapSnapshot {
        ActionMapSnapshot {
            revision: self.action_map_revision,
            bindings: self
                .bindings
                .iter()
                .map(|(&(kind, code), &action)| ActionBinding { kind, code, action })
                .collect(),
        }
    }

    pub fn device(&self, device: Handle) -> Option<&InputDeviceDescriptor> {
        self.devices.get(&device)
    }

    pub fn devices(&self) -> impl Iterator<Item = &InputDeviceDescriptor> {
        self.devices.values()
    }

    pub fn is_focused(&self, scope: InputScopeId) -> bool {
        self.focused_scopes.contains(&scope)
    }

    pub fn connect_device(&mut self, descriptor: InputDeviceDescriptor) -> Result<(), InputError> {
        self.ensure_open()?;
        if !descriptor.device.is_valid() || descriptor.mapping.len() > self.config.max_event_bytes {
            return Err(InputError::InvalidDevice);
        }
        if self.devices.contains_key(&descriptor.device) {
            return Err(InputError::DuplicateDevice);
        }
        if self
            .devices
            .keys()
            .any(|device| device.index == descriptor.device.index)
        {
            return Err(InputError::StaleDevice);
        }
        if self.devices.len() == self.max_devices {
            return Err(InputError::DeviceCapacity);
        }
        self.devices.insert(descriptor.device, descriptor);
        Ok(())
    }

    pub fn disconnect_device(
        &mut self,
        device: Handle,
        timestamp_micros: u64,
    ) -> Result<usize, InputError> {
        if !self.devices.contains_key(&device) {
            return Err(InputError::UnknownDevice);
        }
        let pressed = self
            .pressed
            .keys()
            .filter(|(_, pressed_device, _, _)| *pressed_device == device)
            .copied()
            .collect::<Vec<_>>();
        self.preflight_synthetic(&pressed, timestamp_micros)?;
        self.devices.remove(&device);
        self.synthesize_releases(pressed, timestamp_micros)
    }

    pub fn focus_scope(
        &mut self,
        scope: InputScopeId,
        focused: bool,
        timestamp_micros: u64,
    ) -> Result<usize, InputError> {
        self.validate_scope(scope)?;
        if focused {
            self.focused_scopes.insert(scope);
            return Ok(0);
        }
        self.focused_scopes.remove(&scope);
        let pressed = self
            .pressed
            .keys()
            .filter(|(pressed_scope, _, _, _)| *pressed_scope == scope)
            .copied()
            .collect::<Vec<_>>();
        self.preflight_synthetic(&pressed, timestamp_micros)?;
        self.synthesize_releases(pressed, timestamp_micros)
    }

    pub fn push_routed(&mut self, event: NormalizedInputEvent) -> Result<bool, InputError> {
        self.validate_scope(event.scope)?;
        if !event.device.is_valid() {
            return Err(InputError::InvalidDevice);
        }
        if event.sequence <= self.last_sequence {
            return Err(InputError::Sequence);
        }
        if self
            .last_event_timestamps
            .get(&event.scope)
            .is_some_and(|timestamp| event.timestamp_micros < *timestamp)
        {
            return Err(InputError::TimestampBoundary);
        }
        if event.payload.len() > self.config.max_event_bytes {
            return Err(InputError::EventByteCapacity);
        }
        if event.consumed_by_ui || !self.focused_scopes.contains(&event.scope) {
            self.last_sequence = event.sequence;
            self.last_event_timestamps
                .insert(event.scope, event.timestamp_micros);
            return Ok(false);
        }
        if self.queued_events == self.config.max_queued_events {
            return Err(InputError::EventCapacity);
        }
        let next_bytes = self
            .queued_bytes
            .checked_add(event.payload.len())
            .ok_or(InputError::EventByteCapacity)?;
        if next_bytes > self.config.max_queued_bytes {
            return Err(InputError::EventByteCapacity);
        }
        self.last_sequence = event.sequence;
        self.last_event_timestamps
            .insert(event.scope, event.timestamp_micros);
        self.scope_sequences.insert(event.scope, event.sequence);
        self.queued_events += 1;
        self.queued_bytes = next_bytes;
        let pressed_key = (event.scope, event.device, event.kind, event.code);
        if event.value == 0 {
            self.pressed.remove(&pressed_key);
        } else {
            self.pressed.insert(pressed_key, event.value);
        }
        self.queues.entry(event.scope).or_default().push_back(event);
        Ok(true)
    }

    pub fn push_device_event(&mut self, event: NormalizedInputEvent) -> Result<bool, InputError> {
        if !self.devices.contains_key(&event.device) {
            return Err(InputError::UnknownDevice);
        }
        self.push_routed(event)
    }

    pub fn apply_platform_event(
        &mut self,
        event: NormalizedInputEvent,
    ) -> Result<PlatformInputAdmission, InputError> {
        match event.kind {
            PLATFORM_INPUT_GAMEPAD_CONNECT => {
                self.validate_control_event(&event)?;
                validate_gamepad_descriptor(&event.payload)?;
                self.connect_device(InputDeviceDescriptor {
                    device: event.device,
                    kind: InputDeviceKind::Gamepad,
                    platform_index: event.device.index,
                    mapping: event.payload.clone(),
                })?;
                self.commit_control_event(&event);
                Ok(PlatformInputAdmission::Connected)
            }
            PLATFORM_INPUT_GAMEPAD_DISCONNECT => {
                self.validate_control_event(&event)?;
                if !self.devices.contains_key(&event.device) {
                    return Err(InputError::UnknownDevice);
                }
                let pressed = self
                    .pressed
                    .keys()
                    .filter(|(_, device, _, _)| *device == event.device)
                    .copied()
                    .collect::<Vec<_>>();
                self.preflight_synthetic(&pressed, event.timestamp_micros)?;
                self.devices.remove(&event.device);
                self.commit_control_event(&event);
                let released =
                    self.synthesize_releases_at(pressed, event.timestamp_micros, event.sequence);
                Ok(PlatformInputAdmission::Disconnected { released })
            }
            PLATFORM_INPUT_FOCUS => {
                self.validate_control_event(&event)?;
                let focused = event.value != 0;
                let pressed = if focused {
                    Vec::new()
                } else {
                    self.pressed
                        .keys()
                        .filter(|(scope, _, _, _)| *scope == event.scope)
                        .copied()
                        .collect::<Vec<_>>()
                };
                self.preflight_synthetic(&pressed, event.timestamp_micros)?;
                if focused {
                    self.focused_scopes.insert(event.scope);
                } else {
                    self.focused_scopes.remove(&event.scope);
                }
                self.commit_control_event(&event);
                let released =
                    self.synthesize_releases_at(pressed, event.timestamp_micros, event.sequence);
                Ok(PlatformInputAdmission::FocusChanged { focused, released })
            }
            PLATFORM_INPUT_GAMEPAD_BUTTON | PLATFORM_INPUT_GAMEPAD_AXIS => self
                .push_device_event(event)
                .map(PlatformInputAdmission::Queued),
            _ => self.push_routed(event).map(PlatformInputAdmission::Queued),
        }
    }

    pub fn capture_tick(
        &mut self,
        scope: InputScopeId,
        tick_id: u64,
    ) -> Result<TickInputFrame, InputError> {
        let through_timestamp_micros = self
            .queues
            .get(&scope)
            .and_then(|queue| queue.back())
            .map(|event| event.timestamp_micros)
            .or_else(|| self.last_tick_boundaries.get(&scope).copied())
            .unwrap_or(0);
        self.capture_tick_through(scope, tick_id, through_timestamp_micros)
    }

    pub fn capture_tick_through(
        &mut self,
        scope: InputScopeId,
        tick_id: u64,
        through_timestamp_micros: u64,
    ) -> Result<TickInputFrame, InputError> {
        self.validate_scope(scope)?;
        let last_tick = self.last_ticks.get(&scope).copied().unwrap_or(0);
        if tick_id != last_tick.checked_add(1).ok_or(InputError::TickSequence)? {
            return Err(InputError::TickSequence);
        }
        let previous_boundary = self.last_tick_boundaries.get(&scope).copied().unwrap_or(0);
        if through_timestamp_micros < previous_boundary {
            return Err(InputError::TimestampBoundary);
        }
        let mut queue = self.queues.remove(&scope).unwrap_or_default();
        let split = queue
            .iter()
            .position(|event| event.timestamp_micros > through_timestamp_micros)
            .unwrap_or(queue.len());
        let events = queue.drain(..split).collect::<Vec<_>>();
        if !queue.is_empty() {
            self.queues.insert(scope, queue);
        }
        self.queued_events -= events.len();
        self.queued_bytes -= events
            .iter()
            .map(|event| event.payload.len())
            .sum::<usize>();
        let through_sequence = events
            .last()
            .map(|event| event.sequence)
            .or_else(|| self.scope_sequences.get(&scope).copied())
            .unwrap_or(0);
        let mut actions = BTreeMap::new();
        for event in &events {
            if let Some(action) = self.bindings.get(&(event.kind, event.code)) {
                self.action_state.insert((scope, *action), event.value);
            }
        }
        for (&(action_scope, action), &value) in &self.action_state {
            if action_scope == scope {
                actions.insert(action, value);
            }
        }
        self.last_ticks.insert(scope, tick_id);
        self.last_tick_boundaries
            .insert(scope, through_timestamp_micros);
        Ok(TickInputFrame {
            tick_id,
            scope,
            from_timestamp_micros: previous_boundary,
            through_timestamp_micros,
            through_sequence,
            action_map_revision: self.action_map_revision,
            events,
            actions: actions
                .into_iter()
                .map(|(action, value)| ActionSample { action, value })
                .collect(),
        })
    }

    fn preflight_synthetic(
        &self,
        pressed: &[(InputScopeId, Handle, u16, u32)],
        timestamp_micros: u64,
    ) -> Result<(), InputError> {
        let count = pressed.len();
        if self
            .queued_events
            .checked_add(count)
            .is_none_or(|next| next > self.config.max_queued_events)
        {
            return Err(InputError::EventCapacity);
        }
        if count > 0 && self.last_sequence.checked_add(count as u64).is_none() {
            return Err(InputError::SequenceExhausted);
        }
        if pressed.iter().any(|(scope, _, _, _)| {
            self.last_event_timestamps
                .get(scope)
                .is_some_and(|timestamp| timestamp_micros < *timestamp)
        }) {
            return Err(InputError::TimestampBoundary);
        }
        Ok(())
    }

    fn synthesize_releases(
        &mut self,
        pressed: Vec<(InputScopeId, Handle, u16, u32)>,
        timestamp_micros: u64,
    ) -> Result<usize, InputError> {
        let count = pressed.len();
        for (scope, device, kind, code) in pressed {
            self.last_sequence = self
                .last_sequence
                .checked_add(1)
                .ok_or(InputError::SequenceExhausted)?;
            self.scope_sequences.insert(scope, self.last_sequence);
            self.last_event_timestamps.insert(scope, timestamp_micros);
            self.queued_events += 1;
            self.pressed.remove(&(scope, device, kind, code));
            self.queues
                .entry(scope)
                .or_default()
                .push_back(NormalizedInputEvent {
                    sequence: self.last_sequence,
                    timestamp_micros,
                    scope,
                    device,
                    kind,
                    code,
                    value: 0,
                    modifiers: 0,
                    consumed_by_ui: false,
                    payload: Vec::new(),
                });
        }
        Ok(count)
    }

    fn synthesize_releases_at(
        &mut self,
        pressed: Vec<(InputScopeId, Handle, u16, u32)>,
        timestamp_micros: u64,
        sequence: u64,
    ) -> usize {
        let count = pressed.len();
        for (scope, device, kind, code) in pressed {
            self.scope_sequences.insert(scope, sequence);
            self.last_event_timestamps.insert(scope, timestamp_micros);
            self.queued_events += 1;
            self.pressed.remove(&(scope, device, kind, code));
            self.queues
                .entry(scope)
                .or_default()
                .push_back(NormalizedInputEvent {
                    sequence,
                    timestamp_micros,
                    scope,
                    device,
                    kind,
                    code,
                    value: 0,
                    modifiers: 0,
                    consumed_by_ui: false,
                    payload: Vec::new(),
                });
        }
        count
    }

    fn validate_control_event(&self, event: &NormalizedInputEvent) -> Result<(), InputError> {
        self.validate_scope(event.scope)?;
        if !event.device.is_valid() {
            return Err(InputError::InvalidDevice);
        }
        if event.sequence <= self.last_sequence {
            return Err(InputError::Sequence);
        }
        if event.payload.len() > self.config.max_event_bytes {
            return Err(InputError::EventByteCapacity);
        }
        if self
            .last_event_timestamps
            .get(&event.scope)
            .is_some_and(|timestamp| event.timestamp_micros < *timestamp)
        {
            return Err(InputError::TimestampBoundary);
        }
        Ok(())
    }

    fn commit_control_event(&mut self, event: &NormalizedInputEvent) {
        self.last_sequence = event.sequence;
        self.last_event_timestamps
            .insert(event.scope, event.timestamp_micros);
    }

    fn validate_scope(&self, scope: InputScopeId) -> Result<(), InputError> {
        self.ensure_open()?;
        self.validate_scope_identity(scope)?;
        if !self.scopes.contains(&scope) {
            return Err(InputError::UnknownScope);
        }
        Ok(())
    }

    fn validate_scope_identity(&self, scope: InputScopeId) -> Result<(), InputError> {
        self.ensure_open()?;
        if scope.engine != self.engine {
            return Err(InputError::WrongEngine);
        }
        if !scope.handle.is_valid() {
            return Err(InputError::InvalidScope);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), InputError> {
        if self.closed {
            Err(InputError::Closed)
        } else {
            Ok(())
        }
    }
}

fn push_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn validate_gamepad_descriptor(bytes: &[u8]) -> Result<(), InputError> {
    if bytes.len() < 4 {
        return Err(InputError::InvalidDevice);
    }
    let id_len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    let mapping_len = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    if 4_usize
        .checked_add(id_len)
        .and_then(|length| length.checked_add(mapping_len))
        != Some(bytes.len())
    {
        return Err(InputError::InvalidDevice);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn scope(index: u32) -> InputScopeId {
        InputScopeId {
            engine: handle(1),
            handle: handle(index),
        }
    }

    fn event(sequence: u64, scope: InputScopeId, consumed_by_ui: bool) -> NormalizedInputEvent {
        NormalizedInputEvent {
            sequence,
            timestamp_micros: sequence * 1_000,
            scope,
            device: handle(9),
            kind: 1,
            code: 7,
            value: sequence as i32,
            modifiers: 0,
            consumed_by_ui,
            payload: vec![sequence as u8],
        }
    }

    fn domain() -> InputDomain {
        let mut domain = InputDomain::new(
            handle(1),
            InputDomainConfig {
                max_scopes: 2,
                max_queued_events: 4,
                max_queued_bytes: 4,
                max_event_bytes: 2,
                max_bindings: 2,
            },
        )
        .unwrap();
        domain
            .configure_bindings(vec![ActionBinding {
                kind: 1,
                code: 7,
                action: 3,
            }])
            .unwrap();
        domain.register_scope(scope(1)).unwrap();
        domain.register_scope(scope(2)).unwrap();
        domain
    }

    #[test]
    fn ui_consumed_event_never_enters_game_frame_or_action_map() {
        let mut domain = domain();
        assert_eq!(domain.push_routed(event(1, scope(1), true)), Ok(false));
        domain.push_routed(event(2, scope(1), false)).unwrap();
        let frame = domain.capture_tick(scope(1), 1).unwrap();
        assert_eq!(frame.events.len(), 1);
        assert_eq!(frame.events[0].sequence, 2);
        assert_eq!(
            frame.actions,
            vec![ActionSample {
                action: 3,
                value: 2
            }]
        );
    }

    #[test]
    fn ui_only_input_does_not_change_empty_game_frame_bytes() {
        let mut with_ui = domain();
        let mut without_ui = domain();
        with_ui.push_routed(event(1, scope(1), true)).unwrap();
        let first = with_ui.capture_tick(scope(1), 1).unwrap();
        let second = without_ui.capture_tick(scope(1), 1).unwrap();
        assert_eq!(first.deterministic_bytes(), second.deterministic_bytes());
    }

    #[test]
    fn scopes_are_isolated_and_frame_bytes_are_deterministic() {
        let mut first = domain();
        let mut second = domain();
        for input in [event(1, scope(1), false), event(2, scope(2), false)] {
            first.push_routed(input.clone()).unwrap();
            second.push_routed(input).unwrap();
        }
        let alpha = first.capture_tick(scope(1), 1).unwrap();
        let beta = second.capture_tick(scope(1), 1).unwrap();
        assert_eq!(alpha.deterministic_bytes(), beta.deterministic_bytes());
        assert_eq!(
            first.capture_tick(scope(2), 1).unwrap().events[0].sequence,
            2
        );
    }

    #[test]
    fn sequence_or_capacity_failure_does_not_consume_admission_state() {
        let mut domain = domain();
        domain.push_routed(event(1, scope(1), false)).unwrap();
        assert_eq!(
            domain.push_routed(event(1, scope(1), false)),
            Err(InputError::Sequence)
        );
        assert_eq!(domain.capture_tick(scope(1), 1).unwrap().events.len(), 1);
        domain.push_routed(event(2, scope(1), false)).unwrap();
    }
}
