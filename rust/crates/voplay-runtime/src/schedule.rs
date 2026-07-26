use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Stage {
    Startup,
    PreTick,
    Input,
    Gameplay,
    PrePhysics,
    Physics,
    PostPhysics,
    PostTick,
    Extract,
    Frame,
    Shutdown,
}

impl Stage {
    pub(crate) const fn is_tick(self) -> bool {
        matches!(
            self,
            Self::PreTick
                | Self::Input
                | Self::Gameplay
                | Self::PrePhysics
                | Self::Physics
                | Self::PostPhysics
                | Self::PostTick
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccessSet {
    pub simulation_reads: BTreeSet<u32>,
    pub simulation_writes: BTreeSet<u32>,
    pub presentation_reads: BTreeSet<u32>,
    pub presentation_writes: BTreeSet<u32>,
}

impl AccessSet {
    fn conflicts_with(&self, other: &Self) -> bool {
        intersects(&self.simulation_writes, &other.simulation_reads)
            || intersects(&self.simulation_writes, &other.simulation_writes)
            || intersects(&self.simulation_reads, &other.simulation_writes)
            || intersects(&self.presentation_writes, &other.presentation_reads)
            || intersects(&self.presentation_writes, &other.presentation_writes)
            || intersects(&self.presentation_reads, &other.presentation_writes)
    }
}

fn intersects(left: &BTreeSet<u32>, right: &BTreeSet<u32>) -> bool {
    left.iter().any(|item| right.contains(item))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemSpec {
    pub name: String,
    pub stage: Stage,
    pub deterministic: bool,
    pub access: AccessSet,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    EmptyName,
    DuplicateSystem,
    UnknownDependency,
    StageOrderViolation,
    Cycle,
    UnorderedAccessConflict,
    SimulationWriteFromPresentationStage,
    PresentationAccessFromDeterministicTick,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schedule {
    ordered: Vec<SystemSpec>,
    hash: u64,
}

impl Schedule {
    pub fn configure(systems: Vec<SystemSpec>) -> Result<Self, ScheduleError> {
        let mut by_name = BTreeMap::new();
        for system in systems {
            if system.name.is_empty() {
                return Err(ScheduleError::EmptyName);
            }
            validate_access(&system)?;
            if by_name.insert(system.name.clone(), system).is_some() {
                return Err(ScheduleError::DuplicateSystem);
            }
        }

        let mut edges = by_name
            .keys()
            .map(|name| (name.clone(), BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        let names = by_name.keys().cloned().collect::<Vec<_>>();
        for left in &names {
            for right in &names {
                if by_name[left].stage < by_name[right].stage {
                    edges.get_mut(left).unwrap().insert(right.clone());
                }
            }
        }
        for system in by_name.values() {
            for target in &system.before {
                add_declared_edge(&by_name, &mut edges, &system.name, target)?;
            }
            for source in &system.after {
                add_declared_edge(&by_name, &mut edges, source, &system.name)?;
            }
        }

        let order = stable_topological_order(&edges)?;
        for (position, left) in names.iter().enumerate() {
            for right in names.iter().skip(position + 1) {
                if by_name[left].access.conflicts_with(&by_name[right].access)
                    && !reachable(&edges, left, right)
                    && !reachable(&edges, right, left)
                {
                    return Err(ScheduleError::UnorderedAccessConflict);
                }
            }
        }
        let ordered = order
            .into_iter()
            .map(|name| by_name.remove(&name).unwrap())
            .collect::<Vec<_>>();
        let hash = schedule_hash(&ordered);
        Ok(Self { ordered, hash })
    }

    pub fn systems(&self) -> &[SystemSpec] {
        &self.ordered
    }

    pub const fn hash(&self) -> u64 {
        self.hash
    }
}

fn validate_access(system: &SystemSpec) -> Result<(), ScheduleError> {
    if matches!(system.stage, Stage::Extract | Stage::Frame)
        && !system.access.simulation_writes.is_empty()
    {
        return Err(ScheduleError::SimulationWriteFromPresentationStage);
    }
    if system.deterministic
        && system.stage.is_tick()
        && (!system.access.presentation_reads.is_empty()
            || !system.access.presentation_writes.is_empty())
    {
        return Err(ScheduleError::PresentationAccessFromDeterministicTick);
    }
    Ok(())
}

fn add_declared_edge(
    systems: &BTreeMap<String, SystemSpec>,
    edges: &mut BTreeMap<String, BTreeSet<String>>,
    source: &str,
    target: &str,
) -> Result<(), ScheduleError> {
    let Some(source_system) = systems.get(source) else {
        return Err(ScheduleError::UnknownDependency);
    };
    let Some(target_system) = systems.get(target) else {
        return Err(ScheduleError::UnknownDependency);
    };
    if source_system.stage > target_system.stage {
        return Err(ScheduleError::StageOrderViolation);
    }
    edges.get_mut(source).unwrap().insert(target.to_owned());
    Ok(())
}

fn stable_topological_order(
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<String>, ScheduleError> {
    let mut incoming = edges
        .keys()
        .map(|name| (name.clone(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    for targets in edges.values() {
        for target in targets {
            *incoming.get_mut(target).unwrap() += 1;
        }
    }
    let mut ready = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(edges.len());
    while let Some(name) = ready.pop_first() {
        ordered.push(name.clone());
        for target in &edges[&name] {
            let count = incoming.get_mut(target).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert(target.clone());
            }
        }
    }
    if ordered.len() != edges.len() {
        return Err(ScheduleError::Cycle);
    }
    Ok(ordered)
}

fn reachable(edges: &BTreeMap<String, BTreeSet<String>>, source: &str, target: &str) -> bool {
    let mut pending = vec![source];
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }
        for next in &edges[current] {
            if next == target {
                return true;
            }
            pending.push(next);
        }
    }
    false
}

fn schedule_hash(systems: &[SystemSpec]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for system in systems {
        hash = fnv_mix(hash, system.name.as_bytes());
        hash = fnv_mix(hash, &[system.stage as u8, u8::from(system.deterministic)]);
        for set in [
            &system.access.simulation_reads,
            &system.access.simulation_writes,
            &system.access.presentation_reads,
            &system.access.presentation_writes,
        ] {
            for item in set {
                hash = fnv_mix(hash, &item.to_le_bytes());
            }
            hash = fnv_mix(hash, &[0xff]);
        }
    }
    hash
}

fn fnv_mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{InputFrame, SimulationClock};

    fn system(name: &str, stage: Stage) -> SystemSpec {
        SystemSpec {
            name: name.to_owned(),
            stage,
            deterministic: true,
            access: AccessSet::default(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    #[test]
    fn stable_order_and_hash_do_not_follow_registration_order() {
        let mut physics = system("physics", Stage::Physics);
        physics.access.simulation_writes.insert(1);
        let mut gameplay = system("gameplay", Stage::Gameplay);
        gameplay.access.simulation_writes.insert(1);
        let first = Schedule::configure(vec![physics.clone(), gameplay.clone()]).unwrap();
        let second = Schedule::configure(vec![gameplay, physics]).unwrap();
        assert_eq!(first.hash(), second.hash());
        assert_eq!(
            first
                .systems()
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gameplay", "physics"]
        );
    }

    #[test]
    fn cycle_unordered_conflict_and_state_domain_violations_are_rejected() {
        let mut left = system("left", Stage::Gameplay);
        let mut right = system("right", Stage::Gameplay);
        left.access.simulation_writes.insert(1);
        right.access.simulation_reads.insert(1);
        assert_eq!(
            Schedule::configure(vec![left.clone(), right.clone()]),
            Err(ScheduleError::UnorderedAccessConflict)
        );
        left.before.push("right".to_owned());
        right.before.push("left".to_owned());
        assert_eq!(
            Schedule::configure(vec![left, right]),
            Err(ScheduleError::Cycle)
        );

        let mut extract = system("extract", Stage::Extract);
        extract.access.simulation_writes.insert(1);
        assert_eq!(
            Schedule::configure(vec![extract]),
            Err(ScheduleError::SimulationWriteFromPresentationStage)
        );
        let mut tick = system("tick", Stage::Gameplay);
        tick.access.presentation_reads.insert(2);
        assert_eq!(
            Schedule::configure(vec![tick]),
            Err(ScheduleError::PresentationAccessFromDeterministicTick)
        );
    }

    #[test]
    fn ten_thousand_ticks_are_reproducible_across_presentation_pulses() {
        let mut sparse_pulses = SimulationClock::default();
        let mut dense_pulses = SimulationClock::default();
        for tick_id in 1..=10_000 {
            let input = InputFrame {
                tick_id,
                bytes: tick_id.to_le_bytes().to_vec(),
            };
            sparse_pulses.advance(&input, tick_id / 3).unwrap();
            dense_pulses.advance(&input, tick_id / 3).unwrap();
            if tick_id % 10 == 0 {
                sparse_pulses.notify_presentation();
            }
            dense_pulses.notify_presentation();
            dense_pulses.notify_presentation();
        }
        assert_eq!(
            sparse_pulses.deterministic_hash(),
            dense_pulses.deterministic_hash()
        );
    }
}
