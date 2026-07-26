use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FaultPoint {
    ProtocolDecode,
    EndpointQueue,
    WorkerDispatch,
    ResourceAcquire,
    SurfaceSubmit,
    SurfacePresent,
    DeviceOperation,
    AudioOperation,
    Readback,
    FrameCapture,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectedFault {
    RejectBeforeDispatch,
    FailOwner,
    DropLatestOnly,
    OutcomeUnknown,
    SurfaceLost,
    DeviceLost,
    AudioDeviceLost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultRule {
    pub point: FaultPoint,
    pub fault: InjectedFault,
    pub skip: u64,
    pub every: u64,
    pub remaining: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultInjectionConfig {
    pub max_rules: usize,
    pub max_trace_events: usize,
}

impl Default for FaultInjectionConfig {
    fn default() -> Self {
        Self {
            max_rules: 64,
            max_trace_events: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultInjectionError {
    InvalidConfig,
    InvalidRule,
    RuleCapacity,
    DuplicateRule,
    UnknownRule,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FaultInjectionMetrics {
    pub installed_rules: usize,
    pub evaluated: u64,
    pub injected: u64,
    pub exhausted: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultTraceEvent {
    pub sequence: u64,
    pub point: FaultPoint,
    pub fault: InjectedFault,
    pub visit: u64,
    pub remaining_after: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultInjectionOwnerSnapshot {
    pub closed: bool,
    pub active_rules: usize,
    pub retained_trace_events: usize,
    pub dropped_trace_events: u64,
    pub next_trace_sequence: u64,
    pub metrics: FaultInjectionMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultInjectionShutdownReport {
    pub released_rules: usize,
    pub released_trace_events: usize,
}

#[derive(Clone, Copy)]
struct ActiveRule {
    rule: FaultRule,
    visits: u64,
}

pub struct FaultInjector {
    config: FaultInjectionConfig,
    rules: BTreeMap<FaultPoint, ActiveRule>,
    metrics: FaultInjectionMetrics,
    trace: VecDeque<FaultTraceEvent>,
    dropped_trace_events: u64,
    next_trace_sequence: u64,
    closed: bool,
}

impl FaultInjector {
    pub fn new(config: FaultInjectionConfig) -> Result<Self, FaultInjectionError> {
        if config.max_rules == 0
            || config.max_rules > 4096
            || config.max_trace_events == 0
            || config.max_trace_events > 1_000_000
        {
            return Err(FaultInjectionError::InvalidConfig);
        }
        Ok(Self {
            config,
            rules: BTreeMap::new(),
            metrics: FaultInjectionMetrics::default(),
            trace: VecDeque::new(),
            dropped_trace_events: 0,
            next_trace_sequence: 1,
            closed: false,
        })
    }

    pub fn install(&mut self, rule: FaultRule) -> Result<(), FaultInjectionError> {
        self.ensure_open()?;
        if rule.every == 0 || rule.remaining == 0 {
            return Err(FaultInjectionError::InvalidRule);
        }
        if self.rules.contains_key(&rule.point) {
            return Err(FaultInjectionError::DuplicateRule);
        }
        if self.rules.len() == self.config.max_rules {
            return Err(FaultInjectionError::RuleCapacity);
        }
        self.rules
            .insert(rule.point, ActiveRule { rule, visits: 0 });
        self.metrics.installed_rules = self.rules.len();
        Ok(())
    }

    pub fn replace(&mut self, rule: FaultRule) -> Result<(), FaultInjectionError> {
        self.ensure_open()?;
        if rule.every == 0 || rule.remaining == 0 {
            return Err(FaultInjectionError::InvalidRule);
        }
        if !self.rules.contains_key(&rule.point) && self.rules.len() == self.config.max_rules {
            return Err(FaultInjectionError::RuleCapacity);
        }
        self.rules
            .insert(rule.point, ActiveRule { rule, visits: 0 });
        self.metrics.installed_rules = self.rules.len();
        Ok(())
    }

    pub fn remove(&mut self, point: FaultPoint) -> Result<FaultRule, FaultInjectionError> {
        self.ensure_open()?;
        let rule = self
            .rules
            .remove(&point)
            .map(|active| active.rule)
            .ok_or(FaultInjectionError::UnknownRule)?;
        self.metrics.installed_rules = self.rules.len();
        Ok(rule)
    }

    pub fn clear(&mut self) -> usize {
        let removed = self.rules.len();
        self.rules.clear();
        self.metrics.installed_rules = 0;
        removed
    }

    pub fn trigger(&mut self, point: FaultPoint) -> Option<InjectedFault> {
        if self.closed {
            return None;
        }
        self.metrics.evaluated = self.metrics.evaluated.saturating_add(1);
        let active = self.rules.get_mut(&point)?;
        active.visits = active.visits.saturating_add(1);
        if active.visits <= active.rule.skip {
            return None;
        }
        let eligible_visit = active.visits - active.rule.skip - 1;
        if eligible_visit % active.rule.every != 0 {
            return None;
        }
        let fault = active.rule.fault;
        active.rule.remaining -= 1;
        let remaining_after = active.rule.remaining;
        let visit = active.visits;
        self.metrics.injected = self.metrics.injected.saturating_add(1);
        if remaining_after == 0 {
            self.rules.remove(&point);
            self.metrics.installed_rules = self.rules.len();
            self.metrics.exhausted = self.metrics.exhausted.saturating_add(1);
        }
        self.push_trace(point, fault, visit, remaining_after);
        Some(fault)
    }

    pub const fn metrics(&self) -> FaultInjectionMetrics {
        self.metrics
    }

    pub fn rule(&self, point: FaultPoint) -> Option<FaultRule> {
        self.rules.get(&point).map(|active| active.rule)
    }

    pub fn owner_snapshot(&self) -> FaultInjectionOwnerSnapshot {
        FaultInjectionOwnerSnapshot {
            closed: self.closed,
            active_rules: self.rules.len(),
            retained_trace_events: self.trace.len(),
            dropped_trace_events: self.dropped_trace_events,
            next_trace_sequence: self.next_trace_sequence,
            metrics: self.metrics,
        }
    }

    pub fn trace_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> impl Iterator<Item = FaultTraceEvent> + '_ {
        self.trace
            .iter()
            .copied()
            .filter(move |event| event.sequence > sequence)
            .take(limit)
    }

    pub fn clear_trace(&mut self) -> usize {
        let removed = self.trace.len();
        self.trace.clear();
        removed
    }

    pub fn shutdown(&mut self) -> FaultInjectionShutdownReport {
        let report = FaultInjectionShutdownReport {
            released_rules: self.clear(),
            released_trace_events: self.clear_trace(),
        };
        self.closed = true;
        report
    }

    fn ensure_open(&self) -> Result<(), FaultInjectionError> {
        if self.closed {
            Err(FaultInjectionError::Closed)
        } else {
            Ok(())
        }
    }

    fn push_trace(
        &mut self,
        point: FaultPoint,
        fault: InjectedFault,
        visit: u64,
        remaining_after: u64,
    ) {
        if self.next_trace_sequence == u64::MAX {
            self.dropped_trace_events = self.dropped_trace_events.saturating_add(1);
            return;
        }
        if self.trace.len() == self.config.max_trace_events {
            self.trace.pop_front();
            self.dropped_trace_events = self.dropped_trace_events.saturating_add(1);
        }
        let sequence = self.next_trace_sequence;
        self.next_trace_sequence += 1;
        self.trace.push_back(FaultTraceEvent {
            sequence,
            point,
            fault,
            visit,
            remaining_after,
        });
    }
}
