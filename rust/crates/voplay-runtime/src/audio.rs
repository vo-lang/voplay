use std::collections::VecDeque;

use voplay_protocol::{EngineId, Handle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointConfig {
    pub max_events: usize,
    pub max_event_bytes: usize,
    pub defer_while_locked: bool,
}

impl Default for AudioEndpointConfig {
    fn default() -> Self {
        Self {
            max_events: 1024,
            max_event_bytes: 1024 * 1024,
            defer_while_locked: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointState {
    ReadyLocked,
    Active,
    Suspended,
    Lost,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioGestureToken {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub token: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEvent {
    pub engine: EngineId,
    pub sequence: u64,
    pub event_id: u64,
    pub tick_id: u64,
    pub required_audio_control_revision: u64,
    pub deadline_millis: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEventOutcome {
    Executed,
    DroppedBeforeDispatch,
    OutcomeUnknown,
    FailedControlUnavailable,
    AudioLocked,
    DeadlineExceeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEventResult {
    pub sequence: u64,
    pub event_id: u64,
    pub outcome: AudioEventOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub state: AudioEndpointState,
    pub queued_events: usize,
    pub queued_event_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEndpointShutdownReport {
    pub endpoint_generation: Handle,
    pub terminal_events: Vec<AudioEventResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioPoll {
    Dispatch(AudioEvent),
    Terminal(AudioEventResult),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistentSourceRecoveryPolicy {
    ResumeTimeline,
    Restart,
    StopOnRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistentSourceRecoveryAction {
    ResumeAtMillis(u64),
    RestartAtZero,
    Stop,
}

impl PersistentSourceRecoveryPolicy {
    pub const fn action(self, transport_millis: u64) -> PersistentSourceRecoveryAction {
        match self {
            Self::ResumeTimeline => {
                PersistentSourceRecoveryAction::ResumeAtMillis(transport_millis)
            }
            Self::Restart => PersistentSourceRecoveryAction::RestartAtZero,
            Self::StopOnRecovery => PersistentSourceRecoveryAction::Stop,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioError {
    InvalidConfig,
    WrongEngine,
    StaleEndpoint,
    InvalidGesture,
    InvalidState,
    EventSequence,
    EventCapacity,
    UnknownEvent,
    EventNotDispatched,
    EndpointGenerationExhausted,
    ClockRegression,
    Closed,
}

#[derive(Debug)]
struct StagedAudioEvent {
    event: AudioEvent,
    dispatched: bool,
}

pub struct AudioEndpoint {
    engine: EngineId,
    endpoint_generation: Handle,
    config: AudioEndpointConfig,
    state: AudioEndpointState,
    now_millis: u64,
    audio_control_revision: u64,
    last_event_sequence: u64,
    staged_bytes: usize,
    staged: VecDeque<StagedAudioEvent>,
}

impl AudioEndpoint {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: AudioEndpointConfig,
    ) -> Result<Self, AudioError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_events == 0
            || config.max_event_bytes == 0
        {
            return Err(AudioError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            state: AudioEndpointState::ReadyLocked,
            now_millis: 0,
            audio_control_revision: 0,
            last_event_sequence: 0,
            staged_bytes: 0,
            staged: VecDeque::new(),
        })
    }

    pub const fn state(&self) -> AudioEndpointState {
        self.state
    }

    pub const fn endpoint_generation(&self) -> Handle {
        self.endpoint_generation
    }

    pub fn queued_events(&self) -> usize {
        self.staged.len()
    }

    pub const fn queued_event_bytes(&self) -> usize {
        self.staged_bytes
    }

    pub fn owner_snapshot(&self) -> AudioEndpointOwnerSnapshot {
        AudioEndpointOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            state: self.state,
            queued_events: self.staged.len(),
            queued_event_bytes: self.staged_bytes,
        }
    }

    pub fn set_audio_control_revision(&mut self, revision: u64) {
        if self.state == AudioEndpointState::Closed {
            return;
        }
        self.audio_control_revision = self.audio_control_revision.max(revision);
    }

    pub fn advance_time(&mut self, now_millis: u64) -> Result<(), AudioError> {
        if self.state == AudioEndpointState::Closed {
            return Err(AudioError::Closed);
        }
        if now_millis < self.now_millis {
            return Err(AudioError::ClockRegression);
        }
        self.now_millis = now_millis;
        Ok(())
    }

    pub fn next_deadline_millis(&self) -> Option<u64> {
        self.staged
            .iter()
            .map(|staged| staged.event.deadline_millis)
            .min()
    }

    pub fn activate(&mut self, gesture: AudioGestureToken) -> Result<(), AudioError> {
        if gesture.engine != self.engine {
            return Err(AudioError::WrongEngine);
        }
        if gesture.endpoint_generation != self.endpoint_generation {
            return Err(AudioError::StaleEndpoint);
        }
        if !gesture.token.is_valid() {
            return Err(AudioError::InvalidGesture);
        }
        if self.state != AudioEndpointState::ReadyLocked {
            return Err(AudioError::InvalidState);
        }
        self.state = AudioEndpointState::Active;
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), AudioError> {
        if self.state != AudioEndpointState::Active {
            return Err(AudioError::InvalidState);
        }
        self.state = AudioEndpointState::Suspended;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), AudioError> {
        if self.state != AudioEndpointState::Suspended {
            return Err(AudioError::InvalidState);
        }
        self.state = AudioEndpointState::Active;
        Ok(())
    }

    pub fn stage_event(
        &mut self,
        event: AudioEvent,
    ) -> Result<Option<AudioEventResult>, AudioError> {
        if event.engine != self.engine {
            return Err(AudioError::WrongEngine);
        }
        if self.last_event_sequence.checked_add(1) != Some(event.sequence) || event.event_id == 0 {
            return Err(AudioError::EventSequence);
        }
        if self.state == AudioEndpointState::Closed {
            return Err(AudioError::Closed);
        }
        if self.state == AudioEndpointState::Lost {
            return Err(AudioError::InvalidState);
        }
        if self.state == AudioEndpointState::ReadyLocked && !self.config.defer_while_locked {
            self.last_event_sequence = event.sequence;
            return Ok(Some(AudioEventResult {
                sequence: event.sequence,
                event_id: event.event_id,
                outcome: AudioEventOutcome::AudioLocked,
            }));
        }
        let bytes = self
            .staged_bytes
            .checked_add(event.payload.len())
            .filter(|bytes| *bytes <= self.config.max_event_bytes)
            .ok_or(AudioError::EventCapacity)?;
        if self.staged.len() == self.config.max_events {
            return Err(AudioError::EventCapacity);
        }
        self.last_event_sequence = event.sequence;
        self.staged_bytes = bytes;
        self.staged.push_back(StagedAudioEvent {
            event,
            dispatched: false,
        });
        Ok(None)
    }

    pub fn poll(&mut self) -> Option<AudioPoll> {
        let front = self.staged.front_mut()?;
        if front.event.deadline_millis <= self.now_millis {
            let staged = self.staged.pop_front().unwrap();
            self.staged_bytes -= staged.event.payload.len();
            return Some(AudioPoll::Terminal(AudioEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    AudioEventOutcome::OutcomeUnknown
                } else {
                    AudioEventOutcome::DeadlineExceeded
                },
            }));
        }
        if self.state != AudioEndpointState::Active
            || front.dispatched
            || front.event.required_audio_control_revision > self.audio_control_revision
        {
            return None;
        }
        front.dispatched = true;
        Some(AudioPoll::Dispatch(front.event.clone()))
    }

    pub fn complete(
        &mut self,
        sequence: u64,
        outcome: AudioEventOutcome,
    ) -> Result<AudioEventResult, AudioError> {
        let front = self.staged.front().ok_or(AudioError::UnknownEvent)?;
        if front.event.sequence != sequence {
            return Err(AudioError::UnknownEvent);
        }
        if !front.dispatched {
            return Err(AudioError::EventNotDispatched);
        }
        let staged = self.staged.pop_front().unwrap();
        self.staged_bytes -= staged.event.payload.len();
        Ok(AudioEventResult {
            sequence,
            event_id: staged.event.event_id,
            outcome: if staged.event.deadline_millis <= self.now_millis {
                AudioEventOutcome::OutcomeUnknown
            } else {
                outcome
            },
        })
    }

    pub fn restart(&mut self) -> Result<Vec<AudioEventResult>, AudioError> {
        if self.state == AudioEndpointState::Closed {
            return Err(AudioError::Closed);
        }
        let generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioError::EndpointGenerationExhausted)?;
        self.endpoint_generation.generation = generation;
        self.state = AudioEndpointState::ReadyLocked;
        self.staged_bytes = 0;
        Ok(self
            .staged
            .drain(..)
            .map(|staged| AudioEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    AudioEventOutcome::OutcomeUnknown
                } else {
                    AudioEventOutcome::DroppedBeforeDispatch
                },
            })
            .collect())
    }

    pub fn device_lost(&mut self) -> Result<Vec<AudioEventResult>, AudioError> {
        let results = self.restart()?;
        self.state = AudioEndpointState::Lost;
        Ok(results)
    }

    pub fn preflight_shutdown(&self) -> Result<(), AudioError> {
        if self.state == AudioEndpointState::Closed {
            return Ok(());
        }
        self.endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AudioError::EndpointGenerationExhausted)?;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<AudioEndpointShutdownReport, AudioError> {
        if self.state == AudioEndpointState::Closed {
            return Ok(AudioEndpointShutdownReport {
                endpoint_generation: self.endpoint_generation,
                terminal_events: Vec::new(),
            });
        }
        self.preflight_shutdown()?;
        self.endpoint_generation.generation += 1;
        self.state = AudioEndpointState::Closed;
        self.staged_bytes = 0;
        let terminal_events = self
            .staged
            .drain(..)
            .map(|staged| AudioEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    AudioEventOutcome::OutcomeUnknown
                } else {
                    AudioEventOutcome::DroppedBeforeDispatch
                },
            })
            .collect();
        Ok(AudioEndpointShutdownReport {
            endpoint_generation: self.endpoint_generation,
            terminal_events,
        })
    }
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

    fn event(sequence: u64, required_revision: u64, deadline: u64) -> AudioEvent {
        AudioEvent {
            engine: handle(1),
            sequence,
            event_id: 100 + sequence,
            tick_id: sequence,
            required_audio_control_revision: required_revision,
            deadline_millis: deadline,
            payload: vec![sequence as u8],
        }
    }

    fn gesture(generation: Handle) -> AudioGestureToken {
        AudioGestureToken {
            engine: handle(1),
            endpoint_generation: generation,
            token: handle(99),
        }
    }

    #[test]
    fn ready_locked_rejects_or_bounded_defers_by_declared_policy() {
        let mut reject =
            AudioEndpoint::new(handle(1), handle(3), AudioEndpointConfig::default()).unwrap();
        assert_eq!(
            reject
                .stage_event(event(1, 0, 10))
                .unwrap()
                .unwrap()
                .outcome,
            AudioEventOutcome::AudioLocked
        );
        assert!(reject.poll().is_none());

        let mut defer = AudioEndpoint::new(
            handle(1),
            handle(3),
            AudioEndpointConfig {
                max_events: 1,
                max_event_bytes: 1,
                defer_while_locked: true,
            },
        )
        .unwrap();
        defer.stage_event(event(1, 0, 10)).unwrap();
        assert_eq!(
            defer.stage_event(event(2, 0, 10)),
            Err(AudioError::EventCapacity)
        );
        defer.advance_time(10).unwrap();
        assert_eq!(
            defer.poll(),
            Some(AudioPoll::Terminal(AudioEventResult {
                sequence: 1,
                event_id: 101,
                outcome: AudioEventOutcome::DeadlineExceeded,
            }))
        );
    }

    #[test]
    fn control_barrier_dispatch_and_restart_outcomes_are_honest() {
        let mut endpoint = AudioEndpoint::new(
            handle(1),
            handle(3),
            AudioEndpointConfig {
                defer_while_locked: true,
                ..AudioEndpointConfig::default()
            },
        )
        .unwrap();
        endpoint.activate(gesture(handle(3))).unwrap();
        endpoint.stage_event(event(1, 2, 100)).unwrap();
        endpoint.stage_event(event(2, 0, 100)).unwrap();
        assert!(endpoint.poll().is_none());
        endpoint.set_audio_control_revision(2);
        assert!(matches!(endpoint.poll(), Some(AudioPoll::Dispatch(_))));
        assert!(endpoint.poll().is_none());
        assert_eq!(
            endpoint.restart().unwrap(),
            vec![
                AudioEventResult {
                    sequence: 1,
                    event_id: 101,
                    outcome: AudioEventOutcome::OutcomeUnknown,
                },
                AudioEventResult {
                    sequence: 2,
                    event_id: 102,
                    outcome: AudioEventOutcome::DroppedBeforeDispatch,
                },
            ]
        );
        assert_eq!(endpoint.state(), AudioEndpointState::ReadyLocked);
        assert_eq!(endpoint.endpoint_generation().generation, 2);
    }

    #[test]
    fn gesture_identity_state_transitions_and_recovery_policy_are_explicit() {
        let mut endpoint =
            AudioEndpoint::new(handle(1), handle(3), AudioEndpointConfig::default()).unwrap();
        let mut foreign = gesture(handle(3));
        foreign.engine = handle(2);
        assert_eq!(endpoint.activate(foreign), Err(AudioError::WrongEngine));
        endpoint.activate(gesture(handle(3))).unwrap();
        endpoint.suspend().unwrap();
        endpoint.resume().unwrap();
        assert_eq!(
            PersistentSourceRecoveryPolicy::ResumeTimeline.action(42),
            PersistentSourceRecoveryAction::ResumeAtMillis(42)
        );
        assert_eq!(
            PersistentSourceRecoveryPolicy::Restart.action(42),
            PersistentSourceRecoveryAction::RestartAtZero
        );
        assert_eq!(
            PersistentSourceRecoveryPolicy::StopOnRecovery.action(42),
            PersistentSourceRecoveryAction::Stop
        );
    }
}
