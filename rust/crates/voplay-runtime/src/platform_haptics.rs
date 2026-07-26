use std::collections::{BTreeMap, VecDeque};

use voplay_protocol::Handle;

use crate::haptics::{HapticsCommand, HapticsOutcome, RumbleRequest};
use crate::haptics_wire::HapticsResultWire;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformHapticsSubmitError {
    Unsupported,
    DeviceLost,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformHapticsCompletion {
    pub request_id: u64,
    pub device: Handle,
    pub outcome: HapticsOutcome,
}

pub trait PlatformHapticsDriver: Send {
    fn submit(&mut self, request: RumbleRequest) -> Result<(), PlatformHapticsSubmitError>;

    fn cancel(&mut self, request_id: u64, device: Handle);

    fn poll_completion(&mut self) -> Option<PlatformHapticsCompletion>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformHapticsError {
    InvalidConfig,
    DuplicateRequest,
    PendingCapacity,
    ResultCapacity,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformHapticsOwnerSnapshot {
    pub closed: bool,
    pub pending_requests: usize,
    pub queued_results: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformHapticsShutdownReport {
    pub before: PlatformHapticsOwnerSnapshot,
    pub terminal_results: Vec<HapticsResultWire>,
    pub after: PlatformHapticsOwnerSnapshot,
}

pub struct PlatformHapticsAdapter<D: PlatformHapticsDriver> {
    driver: D,
    max_pending: usize,
    max_results: usize,
    pending: BTreeMap<u64, RumbleRequest>,
    results: VecDeque<HapticsResultWire>,
    closed: bool,
}

impl<D: PlatformHapticsDriver> PlatformHapticsAdapter<D> {
    pub fn new(
        driver: D,
        max_pending: usize,
        max_results: usize,
    ) -> Result<Self, PlatformHapticsError> {
        if max_pending == 0 || max_results == 0 {
            return Err(PlatformHapticsError::InvalidConfig);
        }
        Ok(Self {
            driver,
            max_pending,
            max_results,
            pending: BTreeMap::new(),
            results: VecDeque::new(),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> PlatformHapticsOwnerSnapshot {
        PlatformHapticsOwnerSnapshot {
            closed: self.closed,
            pending_requests: self.pending.len(),
            queued_results: self.results.len(),
        }
    }

    pub fn accept(&mut self, command: HapticsCommand) -> Result<(), PlatformHapticsError> {
        self.ensure_open()?;
        match command {
            HapticsCommand::Start(request) => self.start(request),
            HapticsCommand::Cancel { request_id, device } => {
                let Some(request) = self.pending.get(&request_id).copied() else {
                    return Ok(());
                };
                if request.device != device {
                    return Ok(());
                }
                self.preflight_results(1)?;
                self.pending.remove(&request_id);
                self.driver.cancel(request_id, device);
                self.results.push_back(HapticsResultWire {
                    request_id,
                    device,
                    outcome: HapticsOutcome::Cancelled,
                });
                Ok(())
            }
        }
    }

    pub fn service(&mut self) -> Result<usize, PlatformHapticsError> {
        self.ensure_open()?;
        let mut completed = 0;
        while let Some(completion) = self.driver.poll_completion() {
            let Some(request) = self.pending.get(&completion.request_id).copied() else {
                continue;
            };
            if request.device != completion.device
                || matches!(
                    completion.outcome,
                    HapticsOutcome::Cancelled | HapticsOutcome::TimedOut
                )
            {
                continue;
            }
            self.preflight_results(1)?;
            self.pending.remove(&completion.request_id);
            self.results.push_back(HapticsResultWire {
                request_id: completion.request_id,
                device: completion.device,
                outcome: completion.outcome,
            });
            completed += 1;
        }
        Ok(completed)
    }

    pub fn next_deadline_millis(&self) -> Option<u64> {
        if self.closed {
            return None;
        }
        self.pending
            .values()
            .map(|request| request.deadline_millis)
            .min()
    }

    pub fn service_deadline(&mut self, now_millis: u64) -> Result<usize, PlatformHapticsError> {
        self.ensure_open()?;
        let expired = self
            .pending
            .values()
            .filter(|request| now_millis >= request.deadline_millis)
            .copied()
            .collect::<Vec<_>>();
        self.preflight_results(expired.len())?;
        for request in &expired {
            self.pending.remove(&request.request_id);
            self.driver.cancel(request.request_id, request.device);
            self.results.push_back(HapticsResultWire {
                request_id: request.request_id,
                device: request.device,
                outcome: HapticsOutcome::TimedOut,
            });
        }
        Ok(expired.len())
    }

    pub fn disconnect_device(&mut self, device: Handle) -> Result<usize, PlatformHapticsError> {
        self.ensure_open()?;
        let requests = self
            .pending
            .values()
            .filter(|request| request.device == device)
            .copied()
            .collect::<Vec<_>>();
        self.preflight_results(requests.len())?;
        for request in &requests {
            self.pending.remove(&request.request_id);
            self.driver.cancel(request.request_id, request.device);
            self.results.push_back(HapticsResultWire {
                request_id: request.request_id,
                device: request.device,
                outcome: HapticsOutcome::DeviceLost,
            });
        }
        Ok(requests.len())
    }

    pub fn poll_result(&mut self) -> Option<HapticsResultWire> {
        if self.closed {
            return None;
        }
        self.results.pop_front()
    }

    pub fn shutdown(&mut self) -> PlatformHapticsShutdownReport {
        let before = self.owner_snapshot();
        if self.closed {
            return PlatformHapticsShutdownReport {
                before,
                terminal_results: Vec::new(),
                after: before,
            };
        }
        let mut terminal_results = self.results.drain(..).collect::<Vec<_>>();
        for request in self.pending.values() {
            self.driver.cancel(request.request_id, request.device);
            terminal_results.push(HapticsResultWire {
                request_id: request.request_id,
                device: request.device,
                outcome: HapticsOutcome::Cancelled,
            });
        }
        self.pending.clear();
        self.closed = true;
        PlatformHapticsShutdownReport {
            before,
            terminal_results,
            after: self.owner_snapshot(),
        }
    }

    pub fn into_driver(mut self) -> D {
        self.shutdown();
        self.driver
    }

    fn start(&mut self, request: RumbleRequest) -> Result<(), PlatformHapticsError> {
        if self.pending.contains_key(&request.request_id) {
            return Err(PlatformHapticsError::DuplicateRequest);
        }
        if self.pending.len() == self.max_pending {
            return Err(PlatformHapticsError::PendingCapacity);
        }
        if self
            .results
            .len()
            .checked_add(self.pending.len())
            .and_then(|count| count.checked_add(1))
            .is_none_or(|count| count > self.max_results)
        {
            return Err(PlatformHapticsError::ResultCapacity);
        }
        match self.driver.submit(request) {
            Ok(()) => {
                self.pending.insert(request.request_id, request);
                Ok(())
            }
            Err(error) => {
                self.preflight_results(1)?;
                self.results.push_back(HapticsResultWire {
                    request_id: request.request_id,
                    device: request.device,
                    outcome: match error {
                        PlatformHapticsSubmitError::Unsupported => HapticsOutcome::Unsupported,
                        PlatformHapticsSubmitError::DeviceLost => HapticsOutcome::DeviceLost,
                        PlatformHapticsSubmitError::Failed => HapticsOutcome::Failed,
                    },
                });
                Ok(())
            }
        }
    }

    fn preflight_results(&self, count: usize) -> Result<(), PlatformHapticsError> {
        if self
            .results
            .len()
            .checked_add(count)
            .is_none_or(|next| next > self.max_results)
        {
            return Err(PlatformHapticsError::ResultCapacity);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), PlatformHapticsError> {
        if self.closed {
            Err(PlatformHapticsError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputScopeId;
    use std::collections::VecDeque;

    struct Driver {
        submit_error: Option<PlatformHapticsSubmitError>,
        completions: VecDeque<PlatformHapticsCompletion>,
        cancelled: Vec<u64>,
    }

    impl PlatformHapticsDriver for Driver {
        fn submit(&mut self, _request: RumbleRequest) -> Result<(), PlatformHapticsSubmitError> {
            self.submit_error.take().map_or(Ok(()), Err)
        }

        fn cancel(&mut self, request_id: u64, _device: Handle) {
            self.cancelled.push(request_id);
        }

        fn poll_completion(&mut self) -> Option<PlatformHapticsCompletion> {
            self.completions.pop_front()
        }
    }

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn request(id: u64) -> RumbleRequest {
        RumbleRequest {
            request_id: id,
            scope: InputScopeId {
                engine: handle(1),
                handle: handle(2),
            },
            device: handle(3),
            duration_millis: 50,
            strong_magnitude: 1,
            weak_magnitude: 0,
            deadline_millis: 100,
        }
    }

    #[test]
    fn submit_failure_is_terminal_and_valid_driver_completion_is_one_shot() {
        let driver = Driver {
            submit_error: Some(PlatformHapticsSubmitError::Unsupported),
            completions: VecDeque::new(),
            cancelled: Vec::new(),
        };
        let mut adapter = PlatformHapticsAdapter::new(driver, 4, 4).unwrap();
        adapter.accept(HapticsCommand::Start(request(1))).unwrap();
        assert_eq!(
            adapter.poll_result().unwrap().outcome,
            HapticsOutcome::Unsupported
        );

        adapter.accept(HapticsCommand::Start(request(2))).unwrap();
        adapter.driver.completions.extend([
            PlatformHapticsCompletion {
                request_id: 2,
                device: handle(99),
                outcome: HapticsOutcome::Succeeded,
            },
            PlatformHapticsCompletion {
                request_id: 2,
                device: handle(3),
                outcome: HapticsOutcome::Succeeded,
            },
        ]);
        assert_eq!(adapter.service(), Ok(1));
        assert_eq!(
            adapter.poll_result().unwrap().outcome,
            HapticsOutcome::Succeeded
        );
        assert_eq!(adapter.service(), Ok(0));
    }

    #[test]
    fn cancel_and_disconnect_release_only_matching_pending_requests() {
        let driver = Driver {
            submit_error: None,
            completions: VecDeque::new(),
            cancelled: Vec::new(),
        };
        let mut adapter = PlatformHapticsAdapter::new(driver, 4, 4).unwrap();
        adapter.accept(HapticsCommand::Start(request(1))).unwrap();
        adapter.accept(HapticsCommand::Start(request(2))).unwrap();
        adapter
            .accept(HapticsCommand::Cancel {
                request_id: 1,
                device: handle(3),
            })
            .unwrap();
        assert_eq!(adapter.disconnect_device(handle(3)), Ok(1));
        assert_eq!(adapter.driver.cancelled, vec![1, 2]);
        assert_eq!(
            adapter.poll_result().unwrap().outcome,
            HapticsOutcome::Cancelled
        );
        assert_eq!(
            adapter.poll_result().unwrap().outcome,
            HapticsOutcome::DeviceLost
        );
    }
}
