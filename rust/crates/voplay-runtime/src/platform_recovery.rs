use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::{
    device_hub::{
        DeviceHub, DeviceHubError, DeviceLossReason, DeviceRef, EngineDeviceLease,
        EngineDeviceState,
    },
    surface::GameSurfaceId,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformRecoveryConfig {
    pub max_active_incidents: usize,
    pub max_queued_work: usize,
}

impl Default for PlatformRecoveryConfig {
    fn default() -> Self {
        Self {
            max_active_incidents: 64,
            max_queued_work: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RecoveryIncidentId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryWorkKind {
    RebindSurface {
        engine: EngineId,
        surface: GameSurfaceId,
    },
    InvalidateDeviceResidency {
        device: DeviceRef,
        previous_generation: u64,
        replacement_generation: u64,
    },
    RecoverEngineRenderer {
        lease: EngineDeviceLease,
        render_endpoint: Handle,
        device_generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryWork {
    pub incident: RecoveryIncidentId,
    pub kind: RecoveryWorkKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformRecoveryError {
    Closed,
    InvalidConfig,
    InvalidIdentity,
    Capacity,
    QueueCapacity,
    DuplicateIncident,
    UnknownIncident,
    WrongIncidentKind,
    UnexpectedCompletion,
    RecoveryIncomplete,
    GenerationExhausted,
    DeviceHub(DeviceHubError),
}

impl From<DeviceHubError> for PlatformRecoveryError {
    fn from(error: DeviceHubError) -> Self {
        Self::DeviceHub(error)
    }
}

#[derive(Clone, Debug)]
enum RecoveryIncident {
    Surface {
        engine: EngineId,
        surface: GameSurfaceId,
        work_issued: bool,
    },
    Renderer {
        lease: EngineDeviceLease,
        work_issued: bool,
    },
    Device {
        device: DeviceRef,
        replacement_generation: u64,
        residency_invalidated: bool,
        pending_engines: BTreeSet<EngineDeviceLease>,
        issued_engines: BTreeSet<EngineDeviceLease>,
    },
}

pub struct PlatformRecoveryCoordinator {
    config: PlatformRecoveryConfig,
    next_incident: u64,
    incidents: BTreeMap<RecoveryIncidentId, RecoveryIncident>,
    work: VecDeque<RecoveryWork>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformRecoveryOwnerSnapshot {
    pub closed: bool,
    pub active_incidents: usize,
    pub queued_work: usize,
    pub pending_engine_recoveries: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformRecoveryShutdownReport {
    pub abandoned_incidents: usize,
    pub cancelled_work: Vec<RecoveryWork>,
    pub abandoned_engine_recoveries: usize,
}

impl PlatformRecoveryCoordinator {
    pub fn new(config: PlatformRecoveryConfig) -> Result<Self, PlatformRecoveryError> {
        if config.max_active_incidents == 0 || config.max_queued_work == 0 {
            return Err(PlatformRecoveryError::InvalidConfig);
        }
        Ok(Self {
            config,
            next_incident: 1,
            incidents: BTreeMap::new(),
            work: VecDeque::new(),
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> PlatformRecoveryOwnerSnapshot {
        PlatformRecoveryOwnerSnapshot {
            closed: self.closed,
            active_incidents: self.incidents.len(),
            queued_work: self.work.len(),
            pending_engine_recoveries: self
                .incidents
                .values()
                .map(|incident| match incident {
                    RecoveryIncident::Renderer { .. } => 1,
                    RecoveryIncident::Device {
                        pending_engines, ..
                    } => pending_engines.len(),
                    RecoveryIncident::Surface { .. } => 0,
                })
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> PlatformRecoveryShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.incidents.clear();
        PlatformRecoveryShutdownReport {
            abandoned_incidents: snapshot.active_incidents,
            cancelled_work: self.work.drain(..).collect(),
            abandoned_engine_recoveries: snapshot.pending_engine_recoveries,
        }
    }

    pub fn report_surface_lost(
        &mut self,
        engine: EngineId,
        surface: GameSurfaceId,
    ) -> Result<RecoveryIncidentId, PlatformRecoveryError> {
        self.ensure_open()?;
        if !engine.is_valid() || surface.engine != engine {
            return Err(PlatformRecoveryError::InvalidIdentity);
        }
        if self.incidents.values().any(|incident| {
            matches!(
                incident,
                RecoveryIncident::Surface {
                    engine: current_engine,
                    surface: current_surface,
                    ..
                } if *current_engine == engine && *current_surface == surface
            )
        }) {
            return Err(PlatformRecoveryError::DuplicateIncident);
        }
        self.require_incident_capacity()?;
        self.require_work_capacity(1)?;
        let incident = self.allocate_incident()?;
        self.incidents.insert(
            incident,
            RecoveryIncident::Surface {
                engine,
                surface,
                work_issued: true,
            },
        );
        self.work.push_back(RecoveryWork {
            incident,
            kind: RecoveryWorkKind::RebindSurface { engine, surface },
        });
        Ok(incident)
    }

    pub fn report_renderer_fault(
        &mut self,
        hub: &mut DeviceHub,
        lease: EngineDeviceLease,
        current_endpoint: Handle,
    ) -> Result<RecoveryIncidentId, PlatformRecoveryError> {
        self.ensure_open()?;
        if self.incidents.values().any(|incident| {
            matches!(
                incident,
                RecoveryIncident::Renderer {
                    lease: current, ..
                } if *current == lease
            )
        }) {
            return Err(PlatformRecoveryError::DuplicateIncident);
        }
        self.require_incident_capacity()?;
        self.require_work_capacity(1)?;
        let replacement_endpoint = hub.report_renderer_fault(lease, current_endpoint)?;
        let status = hub.status(lease)?;
        let incident = self.allocate_incident()?;
        self.incidents.insert(
            incident,
            RecoveryIncident::Renderer {
                lease,
                work_issued: true,
            },
        );
        self.work.push_back(RecoveryWork {
            incident,
            kind: RecoveryWorkKind::RecoverEngineRenderer {
                lease,
                render_endpoint: replacement_endpoint,
                device_generation: status.device_generation,
            },
        });
        Ok(incident)
    }

    pub fn report_device_lost(
        &mut self,
        hub: &mut DeviceHub,
        device: DeviceRef,
        generation: u64,
        reason: DeviceLossReason,
    ) -> Result<RecoveryIncidentId, PlatformRecoveryError> {
        self.ensure_open()?;
        if self.incidents.values().any(|incident| {
            matches!(
                incident,
                RecoveryIncident::Device {
                    device: current, ..
                } if *current == device
            )
        }) {
            return Err(PlatformRecoveryError::DuplicateIncident);
        }
        self.require_incident_capacity()?;
        self.require_work_capacity(1)?;
        hub.report_device_lost(device, generation, reason)?;
        let leases = hub.leases_for_device(device)?;
        let replacement_generation = hub.begin_device_recovery(device)?;
        let incident = self.allocate_incident()?;
        self.incidents.insert(
            incident,
            RecoveryIncident::Device {
                device,
                replacement_generation,
                residency_invalidated: false,
                pending_engines: leases.into_iter().collect(),
                issued_engines: BTreeSet::new(),
            },
        );
        self.work.push_back(RecoveryWork {
            incident,
            kind: RecoveryWorkKind::InvalidateDeviceResidency {
                device,
                previous_generation: generation,
                replacement_generation,
            },
        });
        Ok(incident)
    }

    pub fn take_work(&mut self, max: usize) -> Vec<RecoveryWork> {
        let count = max.min(self.work.len());
        self.work.drain(..count).collect()
    }

    pub fn complete_surface_rebind(
        &mut self,
        incident: RecoveryIncidentId,
        engine: EngineId,
        surface: GameSurfaceId,
    ) -> Result<(), PlatformRecoveryError> {
        self.ensure_open()?;
        match self.incidents.get(&incident) {
            Some(RecoveryIncident::Surface {
                engine: expected_engine,
                surface: expected_surface,
                work_issued,
            }) if *expected_engine == engine && *expected_surface == surface && *work_issued => {
                self.incidents.remove(&incident);
                Ok(())
            }
            Some(RecoveryIncident::Surface { .. }) => {
                Err(PlatformRecoveryError::UnexpectedCompletion)
            }
            Some(_) => Err(PlatformRecoveryError::WrongIncidentKind),
            None => Err(PlatformRecoveryError::UnknownIncident),
        }
    }

    pub fn complete_residency_invalidation(
        &mut self,
        hub: &DeviceHub,
        incident: RecoveryIncidentId,
        device: DeviceRef,
        replacement_generation: u64,
    ) -> Result<(), PlatformRecoveryError> {
        self.ensure_open()?;
        let (pending, expected_generation) = match self.incidents.get(&incident) {
            Some(RecoveryIncident::Device {
                device: expected_device,
                replacement_generation: expected_generation,
                residency_invalidated,
                pending_engines,
                ..
            }) if *expected_device == device && !*residency_invalidated => {
                (pending_engines.clone(), *expected_generation)
            }
            Some(RecoveryIncident::Device { .. }) => {
                return Err(PlatformRecoveryError::UnexpectedCompletion);
            }
            Some(_) => return Err(PlatformRecoveryError::WrongIncidentKind),
            None => return Err(PlatformRecoveryError::UnknownIncident),
        };
        if replacement_generation != expected_generation {
            return Err(PlatformRecoveryError::UnexpectedCompletion);
        }
        self.require_work_capacity(pending.len())?;
        let mut work = Vec::with_capacity(pending.len());
        for lease in &pending {
            let status = hub.status(*lease)?;
            if status.state != EngineDeviceState::AwaitingDeviceRebind
                || status.device_generation != replacement_generation
            {
                return Err(PlatformRecoveryError::RecoveryIncomplete);
            }
            work.push(RecoveryWork {
                incident,
                kind: RecoveryWorkKind::RecoverEngineRenderer {
                    lease: *lease,
                    render_endpoint: status.render_endpoint,
                    device_generation: status.device_generation,
                },
            });
        }
        let record = self
            .incidents
            .get_mut(&incident)
            .expect("device recovery incident remains live");
        let RecoveryIncident::Device {
            residency_invalidated,
            issued_engines,
            ..
        } = record
        else {
            unreachable!("incident kind checked above")
        };
        *residency_invalidated = true;
        issued_engines.extend(pending.iter().copied());
        self.work.extend(work);
        if pending.is_empty() {
            self.incidents.remove(&incident);
        }
        Ok(())
    }

    pub fn complete_engine_recovery(
        &mut self,
        hub: &DeviceHub,
        incident: RecoveryIncidentId,
        lease: EngineDeviceLease,
    ) -> Result<(), PlatformRecoveryError> {
        self.ensure_open()?;
        let status = hub.status(lease)?;
        if status.state != EngineDeviceState::Ready {
            return Err(PlatformRecoveryError::RecoveryIncomplete);
        }
        match self.incidents.get_mut(&incident) {
            Some(RecoveryIncident::Renderer {
                lease: expected,
                work_issued,
            }) if *expected == lease && *work_issued => {
                self.incidents.remove(&incident);
                Ok(())
            }
            Some(RecoveryIncident::Renderer { .. }) => {
                Err(PlatformRecoveryError::UnexpectedCompletion)
            }
            Some(RecoveryIncident::Device {
                residency_invalidated,
                pending_engines,
                issued_engines,
                ..
            }) => {
                if !*residency_invalidated
                    || !issued_engines.remove(&lease)
                    || !pending_engines.remove(&lease)
                {
                    return Err(PlatformRecoveryError::UnexpectedCompletion);
                }
                if pending_engines.is_empty() {
                    self.incidents.remove(&incident);
                }
                Ok(())
            }
            Some(RecoveryIncident::Surface { .. }) => Err(PlatformRecoveryError::WrongIncidentKind),
            None => Err(PlatformRecoveryError::UnknownIncident),
        }
    }

    pub fn active_incidents(&self) -> usize {
        self.incidents.len()
    }

    fn allocate_incident(&mut self) -> Result<RecoveryIncidentId, PlatformRecoveryError> {
        self.ensure_open()?;
        let incident = RecoveryIncidentId(self.next_incident);
        self.next_incident = self
            .next_incident
            .checked_add(1)
            .ok_or(PlatformRecoveryError::GenerationExhausted)?;
        Ok(incident)
    }

    fn require_incident_capacity(&self) -> Result<(), PlatformRecoveryError> {
        self.ensure_open()?;
        if self.incidents.len() == self.config.max_active_incidents {
            return Err(PlatformRecoveryError::Capacity);
        }
        Ok(())
    }

    fn require_work_capacity(&self, additional: usize) -> Result<(), PlatformRecoveryError> {
        self.ensure_open()?;
        if self
            .work
            .len()
            .checked_add(additional)
            .filter(|total| *total <= self.config.max_queued_work)
            .is_none()
        {
            return Err(PlatformRecoveryError::QueueCapacity);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), PlatformRecoveryError> {
        if self.closed {
            Err(PlatformRecoveryError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_hub::{DeviceHubConfig, DeviceHubId};
    use crate::outbox::PresentationDomainId;
    use vo_app_protocol::SurfaceHandle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn surface(engine: EngineId) -> GameSurfaceId {
        GameSurfaceId {
            engine,
            surface: SurfaceHandle {
                index: 20,
                generation: 1,
            },
            domain: PresentationDomainId {
                engine,
                handle: handle(21),
            },
        }
    }

    #[test]
    fn surface_incident_requires_exact_completion_and_clears_once() {
        let engine = handle(1);
        let surface = surface(engine);
        let mut recovery =
            PlatformRecoveryCoordinator::new(PlatformRecoveryConfig::default()).unwrap();
        let incident = recovery.report_surface_lost(engine, surface).unwrap();
        assert_eq!(recovery.active_incidents(), 1);
        assert_eq!(
            recovery.complete_surface_rebind(incident, handle(2), surface),
            Err(PlatformRecoveryError::UnexpectedCompletion)
        );
        let work = recovery.take_work(1);
        assert_eq!(
            work,
            vec![RecoveryWork {
                incident,
                kind: RecoveryWorkKind::RebindSurface { engine, surface },
            }]
        );
        recovery
            .complete_surface_rebind(incident, engine, surface)
            .unwrap();
        assert_eq!(recovery.active_incidents(), 0);
        assert_eq!(
            recovery.complete_surface_rebind(incident, engine, surface),
            Err(PlatformRecoveryError::UnknownIncident)
        );
    }

    #[test]
    fn device_loss_invalidates_residency_before_each_engine_rebind() {
        let mut hub = DeviceHub::new(DeviceHubId(handle(30)), DeviceHubConfig::default()).unwrap();
        let device = hub.register_device().unwrap();
        let first = hub.attach_engine(handle(1), device, handle(10)).unwrap();
        let second = hub.attach_engine(handle(2), device, handle(11)).unwrap();
        let mut recovery =
            PlatformRecoveryCoordinator::new(PlatformRecoveryConfig::default()).unwrap();
        let incident = recovery
            .report_device_lost(&mut hub, device, 1, DeviceLossReason::Reset)
            .unwrap();
        let replacement_generation = hub.device_status(device).unwrap().generation;
        assert_eq!(replacement_generation, 2);
        assert!(matches!(
            recovery.take_work(1).as_slice(),
            [RecoveryWork {
                kind: RecoveryWorkKind::InvalidateDeviceResidency {
                    previous_generation: 1,
                    replacement_generation: 2,
                    ..
                },
                ..
            }]
        ));
        recovery
            .complete_residency_invalidation(&hub, incident, device, replacement_generation)
            .unwrap();
        let work = recovery.take_work(8);
        assert_eq!(work.len(), 2);
        assert_eq!(
            recovery.complete_engine_recovery(&hub, incident, first),
            Err(PlatformRecoveryError::RecoveryIncomplete)
        );

        for work in work {
            let RecoveryWorkKind::RecoverEngineRenderer {
                lease,
                render_endpoint,
                device_generation,
            } = work.kind
            else {
                panic!("expected renderer recovery work");
            };
            hub.complete_device_rebind(lease, device_generation, render_endpoint)
                .unwrap();
            recovery
                .complete_engine_recovery(&hub, incident, lease)
                .unwrap();
        }
        assert_eq!(recovery.active_incidents(), 0);
        assert_eq!(hub.status(first).unwrap().state, EngineDeviceState::Ready);
        assert_eq!(hub.status(second).unwrap().state, EngineDeviceState::Ready);
    }
}
