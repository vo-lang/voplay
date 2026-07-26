use std::collections::VecDeque;

use voplay_protocol::EngineId;

use crate::{
    asset::{AssetRef, AssetScopeId},
    audio::AudioEvent,
    control::{ControlDomain, ControlStateSnapshot},
    haptics::HapticsCommand,
    RenderEvent,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicIoConfig {
    pub max_commands: usize,
    pub max_command_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for LogicIoConfig {
    fn default() -> Self {
        Self {
            max_commands: 4096,
            max_command_bytes: 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogicIoCommand {
    AssetRequest {
        sequence: u64,
        scope: AssetScopeId,
        asset_ref: AssetRef,
        deadline_millis: u64,
    },
    AssetControl {
        sequence: u64,
        payload: Vec<u8>,
    },
    RenderControl(ControlStateSnapshot),
    AudioControl(ControlStateSnapshot),
    RenderEvent(RenderEvent),
    AudioEvent(AudioEvent),
    Haptics(HapticsCommand),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicIoSnapshot {
    pub commands: Vec<LogicIoCommand>,
    pub last_asset_sequence: u64,
    pub last_render_event_sequence: u64,
    pub last_audio_event_sequence: u64,
}

impl LogicIoCommand {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::AssetRequest { .. } => 24,
            Self::AssetControl { payload, .. } => payload.len(),
            Self::RenderControl(snapshot) | Self::AudioControl(snapshot) => snapshot
                .entries
                .iter()
                .map(|entry| entry.descriptor.len().saturating_add(48))
                .sum::<usize>()
                .saturating_add(20),
            Self::RenderEvent(event) => event.payload.len(),
            Self::AudioEvent(event) => event.payload.len(),
            Self::Haptics(HapticsCommand::Start(_)) => 41,
            Self::Haptics(HapticsCommand::Cancel { .. }) => 17,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicIoError {
    Closed,
    InvalidConfig,
    WrongEngine,
    InvalidCommand,
    CommandCapacity,
    CommandByteCapacity,
    TotalByteCapacity,
    Sequence,
}

pub struct LogicIoCommit {
    queue: VecDeque<LogicIoCommand>,
    bytes: usize,
    last_asset_sequence: u64,
    last_render_event_sequence: u64,
    last_audio_event_sequence: u64,
}

pub struct LogicIoOutbox {
    engine: EngineId,
    config: LogicIoConfig,
    queue: VecDeque<LogicIoCommand>,
    bytes: usize,
    last_asset_sequence: u64,
    last_render_event_sequence: u64,
    last_audio_event_sequence: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicIoOwnerSnapshot {
    pub closed: bool,
    pub commands: usize,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicIoShutdownReport {
    pub released_commands: usize,
    pub released_bytes: usize,
}

impl LogicIoOutbox {
    pub fn new(engine: EngineId, config: LogicIoConfig) -> Result<Self, LogicIoError> {
        if !engine.is_valid()
            || config.max_commands == 0
            || config.max_command_bytes == 0
            || config.max_total_bytes == 0
        {
            return Err(LogicIoError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            queue: VecDeque::new(),
            bytes: 0,
            last_asset_sequence: 0,
            last_render_event_sequence: 0,
            last_audio_event_sequence: 0,
            closed: false,
        })
    }

    pub fn prepare(&self, commands: Vec<LogicIoCommand>) -> Result<LogicIoCommit, LogicIoError> {
        self.ensure_open()?;
        if self.queue.len().saturating_add(commands.len()) > self.config.max_commands {
            return Err(LogicIoError::CommandCapacity);
        }
        let mut queue = self.queue.clone();
        let mut bytes = self.bytes;
        let mut last_asset_sequence = self.last_asset_sequence;
        let mut last_render_event_sequence = self.last_render_event_sequence;
        let mut last_audio_event_sequence = self.last_audio_event_sequence;
        for command in commands {
            self.validate(&command)?;
            match &command {
                LogicIoCommand::AssetRequest { sequence, .. }
                | LogicIoCommand::AssetControl { sequence, .. } => {
                    if *sequence <= last_asset_sequence {
                        return Err(LogicIoError::Sequence);
                    }
                    last_asset_sequence = *sequence;
                }
                LogicIoCommand::RenderEvent(event) => {
                    if last_render_event_sequence.checked_add(1) != Some(event.sequence) {
                        return Err(LogicIoError::Sequence);
                    }
                    last_render_event_sequence = event.sequence;
                }
                LogicIoCommand::AudioEvent(event) => {
                    if last_audio_event_sequence.checked_add(1) != Some(event.sequence) {
                        return Err(LogicIoError::Sequence);
                    }
                    last_audio_event_sequence = event.sequence;
                }
                LogicIoCommand::RenderControl(_)
                | LogicIoCommand::AudioControl(_)
                | LogicIoCommand::Haptics(_) => {}
            }
            let command_bytes = command.payload_bytes();
            if command_bytes > self.config.max_command_bytes {
                return Err(LogicIoError::CommandByteCapacity);
            }
            bytes = bytes
                .checked_add(command_bytes)
                .filter(|bytes| *bytes <= self.config.max_total_bytes)
                .ok_or(LogicIoError::TotalByteCapacity)?;
            queue.push_back(command);
        }
        Ok(LogicIoCommit {
            queue,
            bytes,
            last_asset_sequence,
            last_render_event_sequence,
            last_audio_event_sequence,
        })
    }

    pub fn commit(&mut self, commit: LogicIoCommit) -> Result<(), LogicIoError> {
        self.ensure_open()?;
        self.queue = commit.queue;
        self.bytes = commit.bytes;
        self.last_asset_sequence = commit.last_asset_sequence;
        self.last_render_event_sequence = commit.last_render_event_sequence;
        self.last_audio_event_sequence = commit.last_audio_event_sequence;
        Ok(())
    }

    pub fn commands(&self) -> &VecDeque<LogicIoCommand> {
        &self.queue
    }

    pub fn owner_snapshot(&self) -> LogicIoOwnerSnapshot {
        LogicIoOwnerSnapshot {
            closed: self.closed,
            commands: self.queue.len(),
            bytes: self.bytes,
        }
    }

    pub fn shutdown(&mut self) -> LogicIoShutdownReport {
        if self.closed {
            return LogicIoShutdownReport {
                released_commands: 0,
                released_bytes: 0,
            };
        }
        let report = LogicIoShutdownReport {
            released_commands: self.queue.len(),
            released_bytes: self.bytes,
        };
        self.closed = true;
        self.queue = VecDeque::new();
        self.bytes = 0;
        report
    }

    pub fn snapshot(&self) -> LogicIoSnapshot {
        LogicIoSnapshot {
            commands: self.queue.iter().cloned().collect(),
            last_asset_sequence: self.last_asset_sequence,
            last_render_event_sequence: self.last_render_event_sequence,
            last_audio_event_sequence: self.last_audio_event_sequence,
        }
    }

    pub fn prepare_restore(
        &self,
        snapshot: LogicIoSnapshot,
    ) -> Result<LogicIoCommit, LogicIoError> {
        self.ensure_open()?;
        if !self.queue.is_empty()
            || self.last_asset_sequence != 0
            || self.last_render_event_sequence != 0
            || self.last_audio_event_sequence != 0
        {
            return Err(LogicIoError::InvalidCommand);
        }
        let render_count = snapshot
            .commands
            .iter()
            .filter(|command| matches!(command, LogicIoCommand::RenderEvent(_)))
            .count() as u64;
        let audio_count = snapshot
            .commands
            .iter()
            .filter(|command| matches!(command, LogicIoCommand::AudioEvent(_)))
            .count() as u64;
        let first_asset_sequence = snapshot.commands.iter().find_map(|command| match command {
            LogicIoCommand::AssetRequest { sequence, .. }
            | LogicIoCommand::AssetControl { sequence, .. } => Some(*sequence),
            _ => None,
        });
        let mut staging = Self::new(self.engine, self.config)?;
        staging.last_asset_sequence = first_asset_sequence
            .map(|sequence| sequence.checked_sub(1).ok_or(LogicIoError::Sequence))
            .transpose()?
            .unwrap_or(snapshot.last_asset_sequence);
        staging.last_render_event_sequence = snapshot
            .last_render_event_sequence
            .checked_sub(render_count)
            .ok_or(LogicIoError::Sequence)?;
        staging.last_audio_event_sequence = snapshot
            .last_audio_event_sequence
            .checked_sub(audio_count)
            .ok_or(LogicIoError::Sequence)?;
        let commit = staging.prepare(snapshot.commands)?;
        if commit.last_asset_sequence != snapshot.last_asset_sequence
            || commit.last_render_event_sequence != snapshot.last_render_event_sequence
            || commit.last_audio_event_sequence != snapshot.last_audio_event_sequence
        {
            return Err(LogicIoError::Sequence);
        }
        Ok(commit)
    }

    pub fn consume(&mut self, count: usize) -> Result<(), LogicIoError> {
        self.ensure_open()?;
        if count > self.queue.len() {
            return Err(LogicIoError::InvalidCommand);
        }
        for _ in 0..count {
            let command = self.queue.pop_front().unwrap();
            self.bytes -= command.payload_bytes();
        }
        Ok(())
    }

    fn validate(&self, command: &LogicIoCommand) -> Result<(), LogicIoError> {
        match command {
            LogicIoCommand::AssetRequest {
                sequence,
                scope,
                asset_ref,
                ..
            } => {
                if *sequence == 0
                    || scope.engine != self.engine
                    || asset_ref.engine != self.engine
                    || !scope.handle.is_valid()
                    || !asset_ref.handle.is_valid()
                {
                    return Err(LogicIoError::InvalidCommand);
                }
            }
            LogicIoCommand::AssetControl { sequence, payload } => {
                if *sequence == 0 || payload.is_empty() {
                    return Err(LogicIoError::InvalidCommand);
                }
            }
            LogicIoCommand::RenderControl(snapshot) => {
                validate_control(snapshot, self.engine, ControlDomain::Render)?;
            }
            LogicIoCommand::AudioControl(snapshot) => {
                validate_control(snapshot, self.engine, ControlDomain::Audio)?;
            }
            LogicIoCommand::RenderEvent(event) => {
                if event.engine != self.engine
                    || event.sequence == 0
                    || event.event_id == 0
                    || event.deadline_millis == 0
                {
                    return Err(LogicIoError::InvalidCommand);
                }
            }
            LogicIoCommand::AudioEvent(event) => {
                if event.engine != self.engine
                    || event.sequence == 0
                    || event.event_id == 0
                    || event.tick_id == 0
                    || event.deadline_millis == 0
                {
                    return Err(LogicIoError::InvalidCommand);
                }
            }
            LogicIoCommand::Haptics(command) => match command {
                HapticsCommand::Start(request) => {
                    if request.request_id == 0
                        || request.scope.engine != self.engine
                        || !request.scope.handle.is_valid()
                        || !request.device.is_valid()
                        || request.duration_millis == 0
                        || (request.strong_magnitude == 0 && request.weak_magnitude == 0)
                        || request.deadline_millis == 0
                    {
                        return Err(LogicIoError::InvalidCommand);
                    }
                }
                HapticsCommand::Cancel { request_id, device } => {
                    if *request_id == 0 || !device.is_valid() {
                        return Err(LogicIoError::InvalidCommand);
                    }
                }
            },
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), LogicIoError> {
        if self.closed {
            Err(LogicIoError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_control(
    snapshot: &ControlStateSnapshot,
    engine: EngineId,
    domain: ControlDomain,
) -> Result<(), LogicIoError> {
    if snapshot.engine != engine
        || snapshot.domain != domain
        || snapshot.revision == 0
        || snapshot.last_transaction_id == 0
    {
        return Err(LogicIoError::InvalidCommand);
    }
    Ok(())
}
