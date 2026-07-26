use voplay_protocol::{EngineId, Handle};

use crate::{
    animation_presentation::{BoneTransform, PresentationAnimationFrame},
    asset::{AssetRef, ResidencyKey, ResidencyState, ResidencyStore},
    sprite_animation::SpriteAnimationFrame,
    world::WorldEntity,
};

const COMMAND_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AnimationGpuCommand {
    Skinning {
        entity: WorldEntity,
        skeleton: AssetRef,
        mesh: AssetRef,
        material: AssetRef,
        residency_key: ResidencyKey,
        palette: Vec<BoneTransform>,
    },
    Sprite {
        entity: WorldEntity,
        texture: AssetRef,
        material: AssetRef,
        residency_key: ResidencyKey,
        uv_rect: [u32; 4],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationGpuBatch {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub actor: Handle,
    pub submission_id: u64,
    pub presentation_revision: u64,
    pub required_control_revision: u64,
    pub commands: Vec<AnimationGpuCommand>,
    pub encoded: Vec<u8>,
    pub digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationGpuAck {
    pub submission_id: u64,
    pub presentation_revision: u64,
    pub command_count: usize,
    pub encoded_bytes: usize,
    pub digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationGpuBackendFailure {
    RejectedBeforeSubmit,
    OutcomeUnknown,
}

pub trait AnimationGpuBackend {
    fn engine(&self) -> EngineId;
    fn endpoint_generation(&self) -> Handle;
    fn actor(&self) -> Handle;
    fn submit(
        &mut self,
        batch: &AnimationGpuBatch,
    ) -> Result<AnimationGpuAck, AnimationGpuBackendFailure>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationGpuConfig {
    pub max_commands_per_submission: usize,
    pub max_palette_bones_per_submission: usize,
    pub max_encoded_bytes: usize,
}

impl Default for AnimationGpuConfig {
    fn default() -> Self {
        Self {
            max_commands_per_submission: 131_072,
            max_palette_bones_per_submission: 1_000_000,
            max_encoded_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationGpuState {
    Ready,
    RecoveryRequired,
    Closing,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnimationGpuError {
    InvalidConfig,
    InvalidIdentity,
    WrongEngine,
    StaleEndpoint,
    WrongActor,
    InvalidState,
    SubmissionSequence,
    RevisionSequence,
    ControlBarrier,
    CommandCapacity,
    PaletteCapacity,
    EncodingCapacity,
    InvalidPacket,
    ResidencyNotReady,
    BackendRejected,
    OutcomeUnknown,
    AckMismatch,
}

pub struct AnimationGpuAdapter {
    engine: EngineId,
    endpoint_generation: Handle,
    actor: Handle,
    config: AnimationGpuConfig,
    state: AnimationGpuState,
    last_submission_id: u64,
    last_presentation_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationGpuOwnerSnapshot {
    pub state: AnimationGpuState,
    pub last_submission_id: u64,
    pub last_presentation_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationGpuShutdownReport {
    pub final_submission_id: u64,
    pub final_presentation_revision: u64,
    pub required_recovery: bool,
}

impl AnimationGpuAdapter {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        actor: Handle,
        config: AnimationGpuConfig,
    ) -> Result<Self, AnimationGpuError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || !actor.is_valid()
            || config.max_commands_per_submission == 0
            || config.max_palette_bones_per_submission == 0
            || config.max_encoded_bytes == 0
        {
            return Err(AnimationGpuError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            actor,
            config,
            state: AnimationGpuState::Ready,
            last_submission_id: 0,
            last_presentation_revision: 0,
        })
    }

    pub const fn state(&self) -> AnimationGpuState {
        self.state
    }

    pub const fn last_submission_id(&self) -> u64 {
        self.last_submission_id
    }

    pub const fn owner_snapshot(&self) -> AnimationGpuOwnerSnapshot {
        AnimationGpuOwnerSnapshot {
            state: self.state,
            last_submission_id: self.last_submission_id,
            last_presentation_revision: self.last_presentation_revision,
        }
    }

    pub fn begin_close(&mut self) {
        if self.state != AnimationGpuState::Closed {
            self.state = AnimationGpuState::Closing;
        }
    }

    pub fn shutdown(&mut self) -> AnimationGpuShutdownReport {
        let snapshot = self.owner_snapshot();
        self.state = AnimationGpuState::Closed;
        AnimationGpuShutdownReport {
            final_submission_id: snapshot.last_submission_id,
            final_presentation_revision: snapshot.last_presentation_revision,
            required_recovery: snapshot.state == AnimationGpuState::RecoveryRequired,
        }
    }

    pub fn submit(
        &mut self,
        submission_id: u64,
        presentation_revision: u64,
        required_control_revision: u64,
        available_control_revision: u64,
        pose_frame: &PresentationAnimationFrame,
        sprite_frame: &SpriteAnimationFrame,
        residency: &ResidencyStore,
        backend: &mut dyn AnimationGpuBackend,
    ) -> Result<AnimationGpuAck, AnimationGpuError> {
        if self.state != AnimationGpuState::Ready {
            return Err(AnimationGpuError::InvalidState);
        }
        if submission_id
            != self
                .last_submission_id
                .checked_add(1)
                .ok_or(AnimationGpuError::SubmissionSequence)?
        {
            return Err(AnimationGpuError::SubmissionSequence);
        }
        if presentation_revision
            != self
                .last_presentation_revision
                .checked_add(1)
                .ok_or(AnimationGpuError::RevisionSequence)?
            || pose_frame.revision != presentation_revision
            || sprite_frame.revision != presentation_revision
        {
            return Err(AnimationGpuError::RevisionSequence);
        }
        if required_control_revision > available_control_revision {
            return Err(AnimationGpuError::ControlBarrier);
        }
        if residency.engine() != self.engine {
            return Err(AnimationGpuError::WrongEngine);
        }
        if residency.endpoint_generation() != self.endpoint_generation {
            return Err(AnimationGpuError::StaleEndpoint);
        }
        if backend.engine() != self.engine {
            return Err(AnimationGpuError::WrongEngine);
        }
        if backend.endpoint_generation() != self.endpoint_generation {
            return Err(AnimationGpuError::StaleEndpoint);
        }
        if backend.actor() != self.actor {
            return Err(AnimationGpuError::WrongActor);
        }

        let mut commands = Vec::new();
        let mut palette_bones = 0usize;
        for packet in &pose_frame.packets {
            if packet.animator.engine != self.engine
                || packet.animator.endpoint_generation != self.endpoint_generation
                || packet.entity.world.engine != self.engine
            {
                return Err(AnimationGpuError::WrongEngine);
            }
            if packet.culled {
                if !packet.palette.is_empty() || packet.dispatch.is_some() {
                    return Err(AnimationGpuError::InvalidPacket);
                }
                continue;
            }
            let dispatch = packet.dispatch.ok_or(AnimationGpuError::InvalidPacket)?;
            if dispatch.skeleton.engine != self.engine || !dispatch.skeleton.handle.is_valid() {
                return Err(AnimationGpuError::InvalidPacket);
            }
            let expected_palette_bytes = packet
                .palette
                .len()
                .checked_mul(std::mem::size_of::<BoneTransform>())
                .ok_or(AnimationGpuError::PaletteCapacity)?;
            if dispatch.bone_count as usize != packet.palette.len()
                || dispatch.palette_bytes != expected_palette_bytes
            {
                return Err(AnimationGpuError::InvalidPacket);
            }
            ensure_ready(
                residency,
                dispatch.mesh,
                dispatch.material,
                dispatch.residency_key,
            )?;
            palette_bones = palette_bones
                .checked_add(packet.palette.len())
                .ok_or(AnimationGpuError::PaletteCapacity)?;
            if palette_bones > self.config.max_palette_bones_per_submission {
                return Err(AnimationGpuError::PaletteCapacity);
            }
            commands.push(AnimationGpuCommand::Skinning {
                entity: packet.entity,
                skeleton: dispatch.skeleton,
                mesh: dispatch.mesh,
                material: dispatch.material,
                residency_key: dispatch.residency_key,
                palette: packet.palette.clone(),
            });
        }
        for draw in &sprite_frame.draws {
            if draw.animator.engine != self.engine
                || draw.animator.endpoint_generation != self.endpoint_generation
                || draw.entity.world.engine != self.engine
            {
                return Err(AnimationGpuError::WrongEngine);
            }
            ensure_ready(residency, draw.texture, draw.material, draw.residency_key)?;
            commands.push(AnimationGpuCommand::Sprite {
                entity: draw.entity,
                texture: draw.texture,
                material: draw.material,
                residency_key: draw.residency_key,
                uv_rect: draw.uv_rect,
            });
        }
        if commands.len() > self.config.max_commands_per_submission {
            return Err(AnimationGpuError::CommandCapacity);
        }
        commands.sort();
        let encoded = encode_commands(&commands)?;
        if encoded.len() > self.config.max_encoded_bytes {
            return Err(AnimationGpuError::EncodingCapacity);
        }
        verify_encoding(&encoded, commands.len())?;
        let digest = stable_digest(&encoded);
        let batch = AnimationGpuBatch {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            actor: self.actor,
            submission_id,
            presentation_revision,
            required_control_revision,
            commands,
            encoded,
            digest,
        };
        let ack = match backend.submit(&batch) {
            Ok(ack) => ack,
            Err(AnimationGpuBackendFailure::RejectedBeforeSubmit) => {
                return Err(AnimationGpuError::BackendRejected);
            }
            Err(AnimationGpuBackendFailure::OutcomeUnknown) => {
                self.state = AnimationGpuState::RecoveryRequired;
                return Err(AnimationGpuError::OutcomeUnknown);
            }
        };
        let expected = AnimationGpuAck {
            submission_id,
            presentation_revision,
            command_count: batch.commands.len(),
            encoded_bytes: batch.encoded.len(),
            digest: batch.digest,
        };
        if ack != expected {
            self.state = AnimationGpuState::RecoveryRequired;
            return Err(AnimationGpuError::AckMismatch);
        }
        self.last_submission_id = submission_id;
        self.last_presentation_revision = presentation_revision;
        Ok(ack)
    }
}

fn ensure_ready(
    residency: &ResidencyStore,
    first: AssetRef,
    second: AssetRef,
    key: ResidencyKey,
) -> Result<(), AnimationGpuError> {
    if first.engine != residency.engine() || second.engine != residency.engine() {
        return Err(AnimationGpuError::WrongEngine);
    }
    if residency.state(first, key) != Some(ResidencyState::Ready)
        || residency.state(second, key) != Some(ResidencyState::Ready)
    {
        return Err(AnimationGpuError::ResidencyNotReady);
    }
    Ok(())
}

pub fn encode_commands(commands: &[AnimationGpuCommand]) -> Result<Vec<u8>, AnimationGpuError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&COMMAND_SCHEMA_VERSION.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(commands.len())
            .map_err(|_| AnimationGpuError::EncodingCapacity)?
            .to_le_bytes(),
    );
    for command in commands {
        match command {
            AnimationGpuCommand::Skinning {
                entity,
                skeleton,
                mesh,
                material,
                residency_key,
                palette,
            } => {
                bytes.push(1);
                encode_entity(&mut bytes, *entity);
                encode_asset(&mut bytes, *skeleton);
                encode_asset(&mut bytes, *mesh);
                encode_asset(&mut bytes, *material);
                encode_key(&mut bytes, *residency_key);
                bytes.extend_from_slice(
                    &u32::try_from(palette.len())
                        .map_err(|_| AnimationGpuError::EncodingCapacity)?
                        .to_le_bytes(),
                );
                for bone in palette {
                    for value in bone
                        .translation
                        .into_iter()
                        .chain(bone.rotation)
                        .chain(bone.scale)
                    {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            AnimationGpuCommand::Sprite {
                entity,
                texture,
                material,
                residency_key,
                uv_rect,
            } => {
                bytes.push(2);
                encode_entity(&mut bytes, *entity);
                encode_asset(&mut bytes, *texture);
                encode_asset(&mut bytes, *material);
                encode_key(&mut bytes, *residency_key);
                for value in uv_rect {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
    Ok(bytes)
}

pub fn verify_encoding(bytes: &[u8], expected_commands: usize) -> Result<(), AnimationGpuError> {
    if bytes.len() < 8 {
        return Err(AnimationGpuError::InvalidPacket);
    }
    let schema = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if schema != COMMAND_SCHEMA_VERSION || count != expected_commands {
        return Err(AnimationGpuError::InvalidPacket);
    }
    let mut offset = 8usize;
    for _ in 0..count {
        let tag = *bytes.get(offset).ok_or(AnimationGpuError::InvalidPacket)?;
        offset += 1;
        let fixed = match tag {
            1 => 24usize + 16 + 16 + 16 + 24,
            2 => 24usize + 16 + 16 + 24,
            _ => return Err(AnimationGpuError::InvalidPacket),
        };
        offset = offset
            .checked_add(fixed)
            .filter(|offset| *offset <= bytes.len())
            .ok_or(AnimationGpuError::InvalidPacket)?;
        match tag {
            1 => {
                let end = offset
                    .checked_add(4)
                    .filter(|end| *end <= bytes.len())
                    .ok_or(AnimationGpuError::InvalidPacket)?;
                let bones = u32::from_le_bytes(bytes[offset..end].try_into().unwrap()) as usize;
                offset = end
                    .checked_add(
                        bones
                            .checked_mul(10 * 8)
                            .ok_or(AnimationGpuError::InvalidPacket)?,
                    )
                    .filter(|offset| *offset <= bytes.len())
                    .ok_or(AnimationGpuError::InvalidPacket)?;
            }
            2 => {
                offset = offset
                    .checked_add(4 * 4)
                    .filter(|offset| *offset <= bytes.len())
                    .ok_or(AnimationGpuError::InvalidPacket)?;
            }
            _ => unreachable!(),
        }
    }
    if offset != bytes.len() {
        return Err(AnimationGpuError::InvalidPacket);
    }
    Ok(())
}

fn encode_entity(bytes: &mut Vec<u8>, entity: WorldEntity) {
    encode_handle(bytes, entity.world.engine);
    encode_handle(bytes, entity.world.handle);
    encode_handle(bytes, entity.entity);
}

fn encode_asset(bytes: &mut Vec<u8>, asset: AssetRef) {
    encode_handle(bytes, asset.engine);
    encode_handle(bytes, asset.handle);
}

fn encode_key(bytes: &mut Vec<u8>, key: ResidencyKey) {
    encode_handle(bytes, key.device);
    bytes.extend_from_slice(&key.usage.to_le_bytes());
    bytes.extend_from_slice(&key.format.to_le_bytes());
    bytes.extend_from_slice(&key.quality.to_le_bytes());
}

fn encode_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn stable_digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        animation_presentation::{
            GpuSkinningDispatch, PresentationAnimatorId, PresentationPosePacket,
        },
        asset::{ArtifactId, ResidencyConfig},
        sprite_animation::{SpriteAnimatorId, SpriteDrawPacket},
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
            device: handle(20),
            usage: 1,
            format: 2,
            quality: 3,
        }
    }

    fn entity(engine: EngineId, index: u32) -> WorldEntity {
        WorldEntity {
            world: WorldId {
                engine,
                handle: handle(30),
            },
            entity: EntityId {
                index,
                generation: 1,
            },
        }
    }

    fn ready_residency(engine: EngineId, generation: Handle) -> ResidencyStore {
        let mut residency = ResidencyStore::new(
            engine,
            generation,
            ResidencyConfig {
                max_records: 8,
                max_ready_bytes: 4096,
            },
        )
        .unwrap();
        for index in 1u8..=3 {
            residency
                .request(
                    asset(engine, u32::from(index)),
                    ArtifactId([index; 16]),
                    key(),
                    64,
                )
                .unwrap();
            let work = residency.poll_upload().unwrap();
            residency.complete_upload(work, true).unwrap();
        }
        residency
    }

    fn frames(
        engine: EngineId,
        generation: Handle,
    ) -> (PresentationAnimationFrame, SpriteAnimationFrame) {
        let bone = BoneTransform {
            translation: [1, 2, 3],
            rotation: [4, 5, 6, 7],
            scale: [8, 9, 10],
        };
        let pose = PresentationAnimationFrame {
            revision: 1,
            packets: vec![PresentationPosePacket {
                animator: PresentationAnimatorId {
                    engine,
                    endpoint_generation: generation,
                    handle: handle(5),
                },
                entity: entity(engine, 9),
                palette: vec![bone],
                dispatch: Some(GpuSkinningDispatch {
                    skeleton: asset(engine, 3),
                    mesh: asset(engine, 1),
                    material: asset(engine, 2),
                    bone_count: 1,
                    palette_bytes: std::mem::size_of::<BoneTransform>(),
                    residency_key: key(),
                }),
                culled: false,
            }],
        };
        let sprites = SpriteAnimationFrame {
            revision: 1,
            pulse_micros: 100,
            draws: vec![SpriteDrawPacket {
                animator: SpriteAnimatorId {
                    engine,
                    endpoint_generation: generation,
                    handle: handle(6),
                },
                entity: entity(engine, 4),
                clip: 1,
                frame: 2,
                uv_rect: [1, 2, 3, 4],
                texture: asset(engine, 3),
                material: asset(engine, 2),
                residency_key: key(),
            }],
            completions: Vec::new(),
        };
        (pose, sprites)
    }

    struct Backend {
        engine: EngineId,
        generation: Handle,
        actor: Handle,
        failure: Option<AnimationGpuBackendFailure>,
        bad_ack: bool,
        calls: usize,
        last_encoded: Vec<u8>,
    }

    impl AnimationGpuBackend for Backend {
        fn engine(&self) -> EngineId {
            self.engine
        }

        fn endpoint_generation(&self) -> Handle {
            self.generation
        }

        fn actor(&self) -> Handle {
            self.actor
        }

        fn submit(
            &mut self,
            batch: &AnimationGpuBatch,
        ) -> Result<AnimationGpuAck, AnimationGpuBackendFailure> {
            self.calls += 1;
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            assert_eq!(encode_commands(&batch.commands).unwrap(), batch.encoded);
            verify_encoding(&batch.encoded, batch.commands.len()).unwrap();
            self.last_encoded = batch.encoded.clone();
            Ok(AnimationGpuAck {
                submission_id: batch.submission_id,
                presentation_revision: batch.presentation_revision,
                command_count: batch.commands.len(),
                encoded_bytes: batch.encoded.len(),
                digest: if self.bad_ack {
                    batch.digest ^ 1
                } else {
                    batch.digest
                },
            })
        }
    }

    fn backend(engine: EngineId, generation: Handle, actor: Handle) -> Backend {
        Backend {
            engine,
            generation,
            actor,
            failure: None,
            bad_ack: false,
            calls: 0,
            last_encoded: Vec::new(),
        }
    }

    #[test]
    fn skinning_and_sprite_submit_as_one_verified_deterministic_batch() {
        let engine = handle(1);
        let generation = handle(9);
        let actor = handle(50);
        let residency = ready_residency(engine, generation);
        let (pose, sprites) = frames(engine, generation);
        let mut adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut backend = backend(engine, generation, actor);
        let ack = adapter
            .submit(1, 1, 3, 3, &pose, &sprites, &residency, &mut backend)
            .unwrap();
        assert_eq!(ack.command_count, 2);
        assert_eq!(adapter.last_submission_id(), 1);
        assert_eq!(backend.calls, 1);
        assert!(!backend.last_encoded.is_empty());
    }

    #[test]
    fn control_residency_and_actor_preflight_never_call_backend() {
        let engine = handle(1);
        let generation = handle(9);
        let actor = handle(50);
        let mut residency = ready_residency(engine, generation);
        let (pose, sprites) = frames(engine, generation);
        let mut adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut backend = backend(engine, generation, actor);
        assert_eq!(
            adapter.submit(1, 1, 4, 3, &pose, &sprites, &residency, &mut backend),
            Err(AnimationGpuError::ControlBarrier)
        );
        residency.device_lost(key().device);
        assert_eq!(
            adapter.submit(1, 1, 3, 3, &pose, &sprites, &residency, &mut backend),
            Err(AnimationGpuError::ResidencyNotReady)
        );
        assert_eq!(backend.calls, 0);
        assert_eq!(adapter.last_submission_id(), 0);
    }

    #[test]
    fn rejected_submission_can_retry_but_unknown_outcome_requires_recovery() {
        let engine = handle(1);
        let generation = handle(9);
        let actor = handle(50);
        let residency = ready_residency(engine, generation);
        let (pose, sprites) = frames(engine, generation);
        let mut adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut backend = backend(engine, generation, actor);
        backend.failure = Some(AnimationGpuBackendFailure::RejectedBeforeSubmit);
        assert_eq!(
            adapter.submit(1, 1, 0, 0, &pose, &sprites, &residency, &mut backend),
            Err(AnimationGpuError::BackendRejected)
        );
        assert_eq!(adapter.state(), AnimationGpuState::Ready);
        backend.failure = None;
        adapter
            .submit(1, 1, 0, 0, &pose, &sprites, &residency, &mut backend)
            .unwrap();

        let mut next_pose = pose.clone();
        next_pose.revision = 2;
        let mut next_sprites = sprites.clone();
        next_sprites.revision = 2;
        backend.failure = Some(AnimationGpuBackendFailure::OutcomeUnknown);
        assert_eq!(
            adapter.submit(
                2,
                2,
                0,
                0,
                &next_pose,
                &next_sprites,
                &residency,
                &mut backend,
            ),
            Err(AnimationGpuError::OutcomeUnknown)
        );
        assert_eq!(adapter.state(), AnimationGpuState::RecoveryRequired);
        assert_eq!(
            adapter.submit(
                2,
                2,
                0,
                0,
                &next_pose,
                &next_sprites,
                &residency,
                &mut backend,
            ),
            Err(AnimationGpuError::InvalidState)
        );
    }

    #[test]
    fn mismatched_ack_requires_recovery_and_does_not_advance_local_revision() {
        let engine = handle(1);
        let generation = handle(9);
        let actor = handle(50);
        let residency = ready_residency(engine, generation);
        let (pose, sprites) = frames(engine, generation);
        let mut adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut backend = backend(engine, generation, actor);
        backend.bad_ack = true;
        assert_eq!(
            adapter.submit(1, 1, 0, 0, &pose, &sprites, &residency, &mut backend),
            Err(AnimationGpuError::AckMismatch)
        );
        assert_eq!(adapter.state(), AnimationGpuState::RecoveryRequired);
        assert_eq!(adapter.last_submission_id(), 0);
    }

    #[test]
    fn packet_input_permutation_produces_identical_gpu_command_bytes() {
        let engine = handle(1);
        let generation = handle(9);
        let actor = handle(50);
        let residency = ready_residency(engine, generation);
        let (mut first_pose, sprites) = frames(engine, generation);
        let mut second_packet = first_pose.packets[0].clone();
        second_packet.entity = entity(engine, 2);
        second_packet.animator.handle = handle(7);
        first_pose.packets.push(second_packet);
        let mut reversed_pose = first_pose.clone();
        reversed_pose.packets.reverse();

        let mut first_adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut second_adapter =
            AnimationGpuAdapter::new(engine, generation, actor, AnimationGpuConfig::default())
                .unwrap();
        let mut first_backend = backend(engine, generation, actor);
        let mut second_backend = backend(engine, generation, actor);
        first_adapter
            .submit(
                1,
                1,
                0,
                0,
                &first_pose,
                &sprites,
                &residency,
                &mut first_backend,
            )
            .unwrap();
        second_adapter
            .submit(
                1,
                1,
                0,
                0,
                &reversed_pose,
                &sprites,
                &residency,
                &mut second_backend,
            )
            .unwrap();
        assert_eq!(first_backend.last_encoded, second_backend.last_encoded);
    }
}
