use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::{AssetRef, AssetServer, CpuNodeState, ResidencyKey, ResidencyState, ResidencyStore},
    world::WorldEntity,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SpriteAnimatorId {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpritePlaybackMode {
    Loop,
    Once,
    PingPong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteFrameDescriptor {
    pub uv_rect: [u32; 4],
    pub duration_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteClipDescriptor {
    pub id: u32,
    pub mode: SpritePlaybackMode,
    pub frames: Vec<SpriteFrameDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteAnimatorDescriptor {
    pub entity: WorldEntity,
    pub sheet: AssetRef,
    pub texture: AssetRef,
    pub material: AssetRef,
    pub initial_clip: u32,
    pub clips: Vec<SpriteClipDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpritePulseRequest {
    pub animator: SpriteAnimatorId,
    pub switch_clip: Option<u32>,
    pub required_control_revision: u64,
    pub residency_key: ResidencyKey,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteDrawPacket {
    pub animator: SpriteAnimatorId,
    pub entity: WorldEntity,
    pub clip: u32,
    pub frame: u32,
    pub uv_rect: [u32; 4],
    pub texture: AssetRef,
    pub material: AssetRef,
    pub residency_key: ResidencyKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteCompletion {
    pub animator: SpriteAnimatorId,
    pub entity: WorldEntity,
    pub clip: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteAnimationFrame {
    pub revision: u64,
    pub pulse_micros: u64,
    pub draws: Vec<SpriteDrawPacket>,
    pub completions: Vec<SpriteCompletion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteAnimationConfig {
    pub max_animators: usize,
    pub max_clips_per_animator: usize,
    pub max_frames_per_clip: usize,
    pub max_requests_per_pulse: usize,
    pub max_frame_transitions_per_pulse: usize,
    pub max_completions_per_pulse: usize,
}

impl Default for SpriteAnimationConfig {
    fn default() -> Self {
        Self {
            max_animators: 65_536,
            max_clips_per_animator: 256,
            max_frames_per_clip: 65_536,
            max_requests_per_pulse: 65_536,
            max_frame_transitions_per_pulse: 1_000_000,
            max_completions_per_pulse: 65_536,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpriteAnimationError {
    Closed,
    InvalidConfig,
    WrongEngine,
    InvalidDescriptor,
    AnimatorCapacity,
    ClipCapacity,
    FrameCapacity,
    RequestCapacity,
    TransitionCapacity,
    CompletionCapacity,
    DuplicateAnimatorRequest,
    InvalidAnimator,
    InvalidClip,
    StaleEndpoint,
    StaleAssetEndpoint,
    ControlBarrier,
    CpuAssetNotReady,
    GpuAssetNotReady,
    PulseRegression,
    RevisionSequence,
    GenerationExhausted,
}

#[derive(Clone, Debug)]
struct Animator {
    descriptor: SpriteAnimatorDescriptor,
    clips: BTreeMap<u32, SpriteClipDescriptor>,
    clip: u32,
    frame: usize,
    elapsed_micros: u64,
    direction: i8,
    completed: bool,
    last_pulse_micros: Option<u64>,
}

#[derive(Clone, Debug)]
struct AnimatorSlot {
    generation: u32,
    animator: Option<Animator>,
}

pub struct SpriteAnimationSystem {
    engine: EngineId,
    endpoint_generation: Handle,
    config: SpriteAnimationConfig,
    revision: u64,
    slots: Vec<AnimatorSlot>,
    free: Vec<u32>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteAnimationOwnerSnapshot {
    pub closed: bool,
    pub live_animators: usize,
    pub allocated_slots: usize,
    pub free_slots: usize,
    pub clips: usize,
    pub frames: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteAnimationShutdownReport {
    pub released_animators: usize,
    pub released_slots: usize,
    pub released_clips: usize,
    pub released_frames: usize,
}

impl SpriteAnimationSystem {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: SpriteAnimationConfig,
    ) -> Result<Self, SpriteAnimationError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_animators == 0
            || config.max_animators > u32::MAX as usize
            || config.max_clips_per_animator == 0
            || config.max_frames_per_clip == 0
            || config.max_requests_per_pulse == 0
            || config.max_frame_transitions_per_pulse == 0
            || config.max_completions_per_pulse == 0
        {
            return Err(SpriteAnimationError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            revision: 0,
            slots: Vec::new(),
            free: Vec::new(),
            closed: false,
        })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn owner_snapshot(&self) -> SpriteAnimationOwnerSnapshot {
        let mut live_animators = 0;
        let mut clips = 0;
        let mut frames = 0;
        for animator in self.slots.iter().filter_map(|slot| slot.animator.as_ref()) {
            live_animators += 1;
            clips += animator.clips.len();
            frames += animator
                .clips
                .values()
                .map(|clip| clip.frames.len())
                .sum::<usize>();
        }
        SpriteAnimationOwnerSnapshot {
            closed: self.closed,
            live_animators,
            allocated_slots: self.slots.len(),
            free_slots: self.free.len(),
            clips,
            frames,
        }
    }

    pub fn shutdown(&mut self) -> SpriteAnimationShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.slots.clear();
        self.free.clear();
        SpriteAnimationShutdownReport {
            released_animators: snapshot.live_animators,
            released_slots: snapshot.allocated_slots,
            released_clips: snapshot.clips,
            released_frames: snapshot.frames,
        }
    }

    pub fn create_animator(
        &mut self,
        descriptor: SpriteAnimatorDescriptor,
    ) -> Result<SpriteAnimatorId, SpriteAnimationError> {
        self.ensure_open()?;
        let clips = validate_descriptor(self.engine, &self.config, &descriptor)?;
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            if self.slots.len() >= self.config.max_animators {
                return Err(SpriteAnimationError::AnimatorCapacity);
            }
            let index = u32::try_from(self.slots.len())
                .map_err(|_| SpriteAnimationError::AnimatorCapacity)?;
            self.slots.push(AnimatorSlot {
                generation: 1,
                animator: None,
            });
            index
        };
        let initial_clip = descriptor.initial_clip;
        let slot = &mut self.slots[index as usize];
        slot.animator = Some(Animator {
            descriptor,
            clips,
            clip: initial_clip,
            frame: 0,
            elapsed_micros: 0,
            direction: 1,
            completed: false,
            last_pulse_micros: None,
        });
        Ok(SpriteAnimatorId {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        })
    }

    pub fn destroy_animator(&mut self, id: SpriteAnimatorId) -> Result<(), SpriteAnimationError> {
        self.ensure_open()?;
        let index = self.animator_index(id)?;
        let generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(SpriteAnimationError::GenerationExhausted)?;
        self.slots[index].animator = None;
        self.slots[index].generation = generation;
        self.free.push(index as u32);
        Ok(())
    }

    pub fn pulse(
        &mut self,
        revision: u64,
        pulse_micros: u64,
        available_control_revision: u64,
        requests: &[SpritePulseRequest],
        assets: &AssetServer,
        residency: &ResidencyStore,
    ) -> Result<SpriteAnimationFrame, SpriteAnimationError> {
        self.ensure_open()?;
        if revision
            != self
                .revision
                .checked_add(1)
                .ok_or(SpriteAnimationError::RevisionSequence)?
        {
            return Err(SpriteAnimationError::RevisionSequence);
        }
        if requests.len() > self.config.max_requests_per_pulse {
            return Err(SpriteAnimationError::RequestCapacity);
        }
        if assets.engine() != self.engine || residency.engine() != self.engine {
            return Err(SpriteAnimationError::WrongEngine);
        }
        if residency.endpoint_generation() != self.endpoint_generation {
            return Err(SpriteAnimationError::StaleAssetEndpoint);
        }
        let mut seen = BTreeSet::new();
        let mut staged = self.slots.clone();
        let mut transitions = 0usize;
        let mut draws = Vec::new();
        let mut completions = Vec::new();
        for request in requests {
            if !seen.insert(request.animator) {
                return Err(SpriteAnimationError::DuplicateAnimatorRequest);
            }
            if request.required_control_revision > available_control_revision {
                return Err(SpriteAnimationError::ControlBarrier);
            }
            let index = animator_index_in(
                self.engine,
                self.endpoint_generation,
                &staged,
                request.animator,
            )?;
            let animator = staged[index]
                .animator
                .as_mut()
                .ok_or(SpriteAnimationError::InvalidAnimator)?;
            if let Some(last) = animator.last_pulse_micros {
                if pulse_micros < last {
                    return Err(SpriteAnimationError::PulseRegression);
                }
            }
            let delta = animator
                .last_pulse_micros
                .map_or(0, |last| pulse_micros - last);
            animator.last_pulse_micros = Some(pulse_micros);
            if let Some(clip) = request.switch_clip {
                if !animator.clips.contains_key(&clip) {
                    return Err(SpriteAnimationError::InvalidClip);
                }
                animator.clip = clip;
                animator.frame = 0;
                animator.elapsed_micros = 0;
                animator.direction = 1;
                animator.completed = false;
            } else {
                advance_animator(
                    animator,
                    delta,
                    &mut transitions,
                    self.config.max_frame_transitions_per_pulse,
                    request.animator,
                    &mut completions,
                    self.config.max_completions_per_pulse,
                )?;
            }
            if !request.visible {
                continue;
            }
            if assets.node_state(animator.descriptor.sheet) != Some(CpuNodeState::CpuReady) {
                return Err(SpriteAnimationError::CpuAssetNotReady);
            }
            if residency.state(animator.descriptor.texture, request.residency_key)
                != Some(ResidencyState::Ready)
                || residency.state(animator.descriptor.material, request.residency_key)
                    != Some(ResidencyState::Ready)
            {
                return Err(SpriteAnimationError::GpuAssetNotReady);
            }
            let clip = &animator.clips[&animator.clip];
            draws.push(SpriteDrawPacket {
                animator: request.animator,
                entity: animator.descriptor.entity,
                clip: animator.clip,
                frame: animator.frame as u32,
                uv_rect: clip.frames[animator.frame].uv_rect,
                texture: animator.descriptor.texture,
                material: animator.descriptor.material,
                residency_key: request.residency_key,
            });
        }
        self.slots = staged;
        self.revision = revision;
        Ok(SpriteAnimationFrame {
            revision,
            pulse_micros,
            draws,
            completions,
        })
    }

    fn animator_index(&self, id: SpriteAnimatorId) -> Result<usize, SpriteAnimationError> {
        self.ensure_open()?;
        animator_index_in(self.engine, self.endpoint_generation, &self.slots, id)
    }

    fn ensure_open(&self) -> Result<(), SpriteAnimationError> {
        if self.closed {
            Err(SpriteAnimationError::Closed)
        } else {
            Ok(())
        }
    }
}

fn animator_index_in(
    engine: EngineId,
    endpoint_generation: Handle,
    slots: &[AnimatorSlot],
    id: SpriteAnimatorId,
) -> Result<usize, SpriteAnimationError> {
    if id.engine != engine {
        return Err(SpriteAnimationError::WrongEngine);
    }
    if id.endpoint_generation != endpoint_generation {
        return Err(SpriteAnimationError::StaleEndpoint);
    }
    let index = id.handle.index as usize;
    let slot = slots
        .get(index)
        .ok_or(SpriteAnimationError::InvalidAnimator)?;
    if id.handle.generation == 0
        || id.handle.generation != slot.generation
        || slot.animator.is_none()
    {
        return Err(SpriteAnimationError::InvalidAnimator);
    }
    Ok(index)
}

fn validate_descriptor(
    engine: EngineId,
    config: &SpriteAnimationConfig,
    descriptor: &SpriteAnimatorDescriptor,
) -> Result<BTreeMap<u32, SpriteClipDescriptor>, SpriteAnimationError> {
    if descriptor.entity.world.engine != engine
        || descriptor.sheet.engine != engine
        || descriptor.texture.engine != engine
        || descriptor.material.engine != engine
    {
        return Err(SpriteAnimationError::WrongEngine);
    }
    if !descriptor.sheet.handle.is_valid()
        || !descriptor.texture.handle.is_valid()
        || !descriptor.material.handle.is_valid()
        || descriptor.clips.is_empty()
    {
        return Err(SpriteAnimationError::InvalidDescriptor);
    }
    if descriptor.clips.len() > config.max_clips_per_animator {
        return Err(SpriteAnimationError::ClipCapacity);
    }
    let mut clips = BTreeMap::new();
    for clip in &descriptor.clips {
        if clip.id == 0
            || clip.frames.is_empty()
            || clip.frames.iter().any(|frame| {
                frame.duration_micros == 0 || frame.uv_rect[2] == 0 || frame.uv_rect[3] == 0
            })
            || clips.insert(clip.id, clip.clone()).is_some()
        {
            return Err(SpriteAnimationError::InvalidDescriptor);
        }
        if clip.frames.len() > config.max_frames_per_clip {
            return Err(SpriteAnimationError::FrameCapacity);
        }
    }
    if !clips.contains_key(&descriptor.initial_clip) {
        return Err(SpriteAnimationError::InvalidClip);
    }
    Ok(clips)
}

fn advance_animator(
    animator: &mut Animator,
    mut delta: u64,
    transitions: &mut usize,
    max_transitions: usize,
    id: SpriteAnimatorId,
    completions: &mut Vec<SpriteCompletion>,
    max_completions: usize,
) -> Result<(), SpriteAnimationError> {
    if animator.completed {
        return Ok(());
    }
    while delta > 0 {
        let clip = &animator.clips[&animator.clip];
        let remaining = clip.frames[animator.frame].duration_micros - animator.elapsed_micros;
        if delta < remaining {
            animator.elapsed_micros += delta;
            break;
        }
        delta -= remaining;
        animator.elapsed_micros = 0;
        *transitions = transitions
            .checked_add(1)
            .ok_or(SpriteAnimationError::TransitionCapacity)?;
        if *transitions > max_transitions {
            return Err(SpriteAnimationError::TransitionCapacity);
        }
        match clip.mode {
            SpritePlaybackMode::Loop => animator.frame = (animator.frame + 1) % clip.frames.len(),
            SpritePlaybackMode::Once => {
                if animator.frame + 1 == clip.frames.len() {
                    animator.completed = true;
                    if completions.len() >= max_completions {
                        return Err(SpriteAnimationError::CompletionCapacity);
                    }
                    completions.push(SpriteCompletion {
                        animator: id,
                        entity: animator.descriptor.entity,
                        clip: animator.clip,
                    });
                    break;
                }
                animator.frame += 1;
            }
            SpritePlaybackMode::PingPong => {
                if clip.frames.len() > 1 {
                    if animator.direction > 0 && animator.frame + 1 == clip.frames.len() {
                        animator.direction = -1;
                    } else if animator.direction < 0 && animator.frame == 0 {
                        animator.direction = 1;
                    }
                    animator.frame = animator
                        .frame
                        .saturating_add_signed(animator.direction.into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        asset::{
            ArtifactId, AssetId, AssetRegistration, AssetScopeKind, AssetServerConfig,
            ResidencyConfig,
        },
        world::WorldId,
    };
    use voplay_protocol::EntityId;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn asset(engine: EngineId, index: u32) -> AssetRef {
        AssetRef {
            engine,
            handle: handle(index),
        }
    }

    fn key() -> ResidencyKey {
        ResidencyKey {
            device: handle(40),
            usage: 1,
            format: 2,
            quality: 3,
        }
    }

    fn descriptor(engine: EngineId) -> SpriteAnimatorDescriptor {
        SpriteAnimatorDescriptor {
            entity: WorldEntity {
                world: WorldId {
                    engine,
                    handle: handle(30),
                },
                entity: EntityId {
                    index: 4,
                    generation: 2,
                },
            },
            sheet: asset(engine, 0),
            texture: asset(engine, 1),
            material: asset(engine, 2),
            initial_clip: 1,
            clips: vec![
                SpriteClipDescriptor {
                    id: 1,
                    mode: SpritePlaybackMode::Loop,
                    frames: vec![
                        SpriteFrameDescriptor {
                            uv_rect: [0, 0, 10, 10],
                            duration_micros: 10,
                        },
                        SpriteFrameDescriptor {
                            uv_rect: [10, 0, 10, 10],
                            duration_micros: 20,
                        },
                    ],
                },
                SpriteClipDescriptor {
                    id: 2,
                    mode: SpritePlaybackMode::Once,
                    frames: vec![
                        SpriteFrameDescriptor {
                            uv_rect: [0, 10, 10, 10],
                            duration_micros: 5,
                        },
                        SpriteFrameDescriptor {
                            uv_rect: [10, 10, 10, 10],
                            duration_micros: 5,
                        },
                    ],
                },
                SpriteClipDescriptor {
                    id: 3,
                    mode: SpritePlaybackMode::PingPong,
                    frames: vec![
                        SpriteFrameDescriptor {
                            uv_rect: [0, 20, 10, 10],
                            duration_micros: 5,
                        },
                        SpriteFrameDescriptor {
                            uv_rect: [10, 20, 10, 10],
                            duration_micros: 5,
                        },
                        SpriteFrameDescriptor {
                            uv_rect: [20, 20, 10, 10],
                            duration_micros: 5,
                        },
                    ],
                },
            ],
        }
    }

    fn ready_stores(engine: EngineId, generation: Handle) -> (AssetServer, ResidencyStore) {
        let mut assets =
            AssetServer::new(engine, handle(20), AssetServerConfig::default()).unwrap();
        let refs = assets
            .register_batch(
                (0u8..3)
                    .map(|index| AssetRegistration {
                        asset_id: AssetId([index + 1; 16]),
                        asset_type: u64::from(index + 1),
                        source_revision: 1,
                        artifact_id: ArtifactId([index + 11; 16]),
                        dependencies: Vec::new(),
                    })
                    .collect(),
            )
            .unwrap();
        let scope = assets.create_scope(AssetScopeKind::Engine).unwrap();
        for asset in refs {
            assets.request(scope, asset, 100).unwrap();
        }
        while let Some(work) = assets.poll_work() {
            assets.complete(&work).unwrap();
        }
        let mut residency = ResidencyStore::new(
            engine,
            generation,
            ResidencyConfig {
                max_records: 8,
                max_ready_bytes: 1024,
            },
        )
        .unwrap();
        for (asset, artifact) in [(asset(engine, 1), 12), (asset(engine, 2), 13)] {
            residency
                .request(asset, ArtifactId([artifact; 16]), key(), 64)
                .unwrap();
            let work = residency.poll_upload().unwrap();
            residency.complete_upload(work, true).unwrap();
        }
        (assets, residency)
    }

    fn request(animator: SpriteAnimatorId) -> SpritePulseRequest {
        SpritePulseRequest {
            animator,
            switch_clip: None,
            required_control_revision: 1,
            residency_key: key(),
            visible: true,
        }
    }

    #[test]
    fn variable_duration_loop_and_ping_pong_follow_presentation_pulses() {
        let engine = handle(1);
        let generation = handle(9);
        let (assets, residency) = ready_stores(engine, generation);
        let mut system =
            SpriteAnimationSystem::new(engine, generation, SpriteAnimationConfig::default())
                .unwrap();
        let animator = system.create_animator(descriptor(engine)).unwrap();
        assert_eq!(
            system
                .pulse(1, 100, 1, &[request(animator)], &assets, &residency)
                .unwrap()
                .draws[0]
                .frame,
            0
        );
        assert_eq!(
            system
                .pulse(2, 115, 1, &[request(animator)], &assets, &residency)
                .unwrap()
                .draws[0]
                .frame,
            1
        );
        let mut switch = request(animator);
        switch.switch_clip = Some(3);
        system
            .pulse(3, 120, 1, &[switch], &assets, &residency)
            .unwrap();
        assert_eq!(
            system
                .pulse(4, 130, 1, &[request(animator)], &assets, &residency)
                .unwrap()
                .draws[0]
                .frame,
            2
        );
        assert_eq!(
            system
                .pulse(5, 135, 1, &[request(animator)], &assets, &residency)
                .unwrap()
                .draws[0]
                .frame,
            1
        );
    }

    #[test]
    fn once_completion_is_emitted_exactly_once_until_clip_restart() {
        let engine = handle(1);
        let generation = handle(9);
        let (assets, residency) = ready_stores(engine, generation);
        let mut system =
            SpriteAnimationSystem::new(engine, generation, SpriteAnimationConfig::default())
                .unwrap();
        let animator = system.create_animator(descriptor(engine)).unwrap();
        let mut switch = request(animator);
        switch.switch_clip = Some(2);
        system
            .pulse(1, 0, 1, &[switch], &assets, &residency)
            .unwrap();
        let completed = system
            .pulse(2, 10, 1, &[request(animator)], &assets, &residency)
            .unwrap();
        assert_eq!(completed.completions.len(), 1);
        assert_eq!(
            system
                .pulse(3, 100, 1, &[request(animator)], &assets, &residency)
                .unwrap()
                .completions
                .len(),
            0
        );
    }

    #[test]
    fn culled_sprite_advances_without_ready_assets_or_draw_packet() {
        let engine = handle(1);
        let generation = handle(9);
        let (mut assets, mut residency) = ready_stores(engine, generation);
        let mut system =
            SpriteAnimationSystem::new(engine, generation, SpriteAnimationConfig::default())
                .unwrap();
        let animator = system.create_animator(descriptor(engine)).unwrap();
        let mut hidden = request(animator);
        hidden.visible = false;
        system
            .pulse(1, 0, 1, &[hidden], &assets, &residency)
            .unwrap();
        assets
            .hot_reload(AssetRegistration {
                asset_id: AssetId([1; 16]),
                asset_type: 1,
                source_revision: 2,
                artifact_id: ArtifactId([99; 16]),
                dependencies: Vec::new(),
            })
            .unwrap();
        residency.device_lost(key().device);
        assert!(system
            .pulse(2, 15, 1, &[hidden], &assets, &residency)
            .unwrap()
            .draws
            .is_empty());
        let frame = system
            .pulse(3, 30, 1, &[hidden], &assets, &residency)
            .unwrap();
        assert!(frame.draws.is_empty());
    }

    #[test]
    fn control_residency_and_time_failures_leave_revision_and_timeline_unchanged() {
        let engine = handle(1);
        let generation = handle(9);
        let (assets, mut residency) = ready_stores(engine, generation);
        let mut system =
            SpriteAnimationSystem::new(engine, generation, SpriteAnimationConfig::default())
                .unwrap();
        let animator = system.create_animator(descriptor(engine)).unwrap();
        system
            .pulse(1, 10, 1, &[request(animator)], &assets, &residency)
            .unwrap();
        assert_eq!(
            system.pulse(2, 9, 1, &[request(animator)], &assets, &residency),
            Err(SpriteAnimationError::PulseRegression)
        );
        let mut blocked = request(animator);
        blocked.required_control_revision = 2;
        assert_eq!(
            system.pulse(2, 20, 1, &[blocked], &assets, &residency),
            Err(SpriteAnimationError::ControlBarrier)
        );
        residency.device_lost(key().device);
        assert_eq!(
            system.pulse(2, 20, 1, &[request(animator)], &assets, &residency),
            Err(SpriteAnimationError::GpuAssetNotReady)
        );
        assert_eq!(system.revision(), 1);
    }

    #[test]
    fn destroyed_and_foreign_animator_handles_fail_before_mutation() {
        let engine = handle(1);
        let generation = handle(9);
        let (assets, residency) = ready_stores(engine, generation);
        let mut system =
            SpriteAnimationSystem::new(engine, generation, SpriteAnimationConfig::default())
                .unwrap();
        let first = system.create_animator(descriptor(engine)).unwrap();
        system.destroy_animator(first).unwrap();
        let current = system.create_animator(descriptor(engine)).unwrap();
        assert_eq!(
            system.pulse(1, 0, 1, &[request(first)], &assets, &residency),
            Err(SpriteAnimationError::InvalidAnimator)
        );
        let foreign = SpriteAnimatorId {
            engine: handle(2),
            ..current
        };
        assert_eq!(
            system.pulse(1, 0, 1, &[request(foreign)], &assets, &residency),
            Err(SpriteAnimationError::WrongEngine)
        );
        assert_eq!(system.revision(), 0);
    }
}
