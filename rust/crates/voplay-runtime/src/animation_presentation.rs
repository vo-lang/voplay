use std::collections::BTreeSet;

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::{AssetRef, AssetServer, CpuNodeState, ResidencyKey, ResidencyState, ResidencyStore},
    world::WorldEntity,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PresentationAnimatorId {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationAssetBinding {
    pub skeleton: AssetRef,
    pub graph: AssetRef,
    pub clips: Vec<AssetRef>,
    pub masks: Vec<AssetRef>,
    pub mesh: AssetRef,
    pub material: AssetRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationAnimatorDescriptor {
    pub entity: WorldEntity,
    pub assets: AnimationAssetBinding,
    pub bone_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BoneTransform {
    pub translation: [i64; 3],
    pub rotation: [i64; 4],
    pub scale: [i64; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoseSampleRequest {
    pub animator: PresentationAnimatorId,
    pub previous_clip: AssetRef,
    pub previous_tick: u64,
    pub current_clip: AssetRef,
    pub current_tick: u64,
    pub interpolation_numerator: u32,
    pub interpolation_denominator: u32,
    pub required_control_revision: u64,
    pub residency_key: ResidencyKey,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationBlendLayer {
    pub clip: AssetRef,
    pub tick: u64,
    pub weight_numerator: u32,
    pub weight_denominator: u32,
    pub mask: Option<AssetRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoseProviderError {
    MissingArtifact,
    InvalidArtifact,
    DecodeFailed,
    Capacity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoseProviderState {
    Ready,
    Closing,
    Closed,
}

pub trait PoseSamplingProvider {
    fn engine(&self) -> EngineId;

    fn endpoint_generation(&self) -> Handle;

    fn state(&self) -> PoseProviderState;

    fn evaluate_graph(
        &self,
        _graph: AssetRef,
        _request: PoseSampleRequest,
    ) -> Result<Vec<AnimationBlendLayer>, PoseProviderError> {
        Ok(Vec::new())
    }

    fn sample_pose(
        &self,
        skeleton: AssetRef,
        clip: AssetRef,
        tick: u64,
        bone_count: u32,
    ) -> Result<Vec<BoneTransform>, PoseProviderError>;

    fn sample_mask(
        &self,
        _mask: AssetRef,
        _bone_count: u32,
    ) -> Result<Vec<u16>, PoseProviderError> {
        Err(PoseProviderError::InvalidArtifact)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuSkinningDispatch {
    pub skeleton: AssetRef,
    pub mesh: AssetRef,
    pub material: AssetRef,
    pub bone_count: u32,
    pub palette_bytes: usize,
    pub residency_key: ResidencyKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationPosePacket {
    pub animator: PresentationAnimatorId,
    pub entity: WorldEntity,
    pub palette: Vec<BoneTransform>,
    pub dispatch: Option<GpuSkinningDispatch>,
    pub culled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationAnimationFrame {
    pub revision: u64,
    pub packets: Vec<PresentationPosePacket>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationAnimationConfig {
    pub max_animators: usize,
    pub max_bones_per_animator: usize,
    pub max_clips_per_animator: usize,
    pub max_masks_per_animator: usize,
    pub max_graph_layers_per_animator: usize,
    pub max_requests_per_frame: usize,
    pub max_palette_bytes_per_frame: usize,
}

impl Default for PresentationAnimationConfig {
    fn default() -> Self {
        Self {
            max_animators: 65_536,
            max_bones_per_animator: 1_024,
            max_clips_per_animator: 256,
            max_masks_per_animator: 64,
            max_graph_layers_per_animator: 64,
            max_requests_per_frame: 65_536,
            max_palette_bytes_per_frame: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresentationAnimationError {
    Closed,
    InvalidConfig,
    WrongEngine,
    InvalidDescriptor,
    AnimatorCapacity,
    BoneCapacity,
    AssetCapacity,
    RequestCapacity,
    PaletteCapacity,
    DuplicateAnimatorRequest,
    InvalidAnimator,
    StaleEndpoint,
    InvalidInterpolation,
    ClipNotBound,
    MaskNotBound,
    GraphCapacity,
    ControlBarrier,
    CpuAssetNotReady,
    GpuAssetNotReady,
    StaleAssetEndpoint,
    StaleProvider,
    ProviderUnavailable,
    Provider(PoseProviderError),
    ProviderContract,
    ArithmeticOverflow,
    RevisionSequence,
    GenerationExhausted,
}

#[derive(Clone, Debug)]
struct AnimatorSlot {
    generation: u32,
    descriptor: Option<PresentationAnimatorDescriptor>,
}

pub struct PresentationAnimationEndpoint {
    engine: EngineId,
    endpoint_generation: Handle,
    config: PresentationAnimationConfig,
    revision: u64,
    slots: Vec<AnimatorSlot>,
    free: Vec<u32>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationAnimationOwnerSnapshot {
    pub closed: bool,
    pub live_animators: usize,
    pub allocated_slots: usize,
    pub free_slots: usize,
    pub bound_assets: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationAnimationShutdownReport {
    pub released_animators: usize,
    pub released_slots: usize,
    pub released_bound_assets: usize,
}

impl PresentationAnimationEndpoint {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: PresentationAnimationConfig,
    ) -> Result<Self, PresentationAnimationError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_animators == 0
            || config.max_animators > u32::MAX as usize
            || config.max_bones_per_animator == 0
            || config.max_clips_per_animator == 0
            || config.max_masks_per_animator == 0
            || config.max_graph_layers_per_animator == 0
            || config.max_requests_per_frame == 0
            || config.max_palette_bytes_per_frame == 0
        {
            return Err(PresentationAnimationError::InvalidConfig);
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

    pub fn owner_snapshot(&self) -> PresentationAnimationOwnerSnapshot {
        let mut live_animators = 0;
        let mut bound_assets = 0;
        for descriptor in self
            .slots
            .iter()
            .filter_map(|slot| slot.descriptor.as_ref())
        {
            live_animators += 1;
            bound_assets += 4 + descriptor.assets.clips.len() + descriptor.assets.masks.len();
        }
        PresentationAnimationOwnerSnapshot {
            closed: self.closed,
            live_animators,
            allocated_slots: self.slots.len(),
            free_slots: self.free.len(),
            bound_assets,
        }
    }

    pub fn shutdown(&mut self) -> PresentationAnimationShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.slots.clear();
        self.free.clear();
        PresentationAnimationShutdownReport {
            released_animators: snapshot.live_animators,
            released_slots: snapshot.allocated_slots,
            released_bound_assets: snapshot.bound_assets,
        }
    }

    pub fn create_animator(
        &mut self,
        descriptor: PresentationAnimatorDescriptor,
    ) -> Result<PresentationAnimatorId, PresentationAnimationError> {
        self.ensure_open()?;
        validate_descriptor(self.engine, &self.config, &descriptor)?;
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            if self.slots.len() >= self.config.max_animators {
                return Err(PresentationAnimationError::AnimatorCapacity);
            }
            let index = u32::try_from(self.slots.len())
                .map_err(|_| PresentationAnimationError::AnimatorCapacity)?;
            self.slots.push(AnimatorSlot {
                generation: 1,
                descriptor: None,
            });
            index
        };
        let slot = &mut self.slots[index as usize];
        slot.descriptor = Some(descriptor);
        Ok(PresentationAnimatorId {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        })
    }

    pub fn destroy_animator(
        &mut self,
        animator: PresentationAnimatorId,
    ) -> Result<(), PresentationAnimationError> {
        self.ensure_open()?;
        let index = self.animator_index(animator)?;
        let next_generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(PresentationAnimationError::GenerationExhausted)?;
        self.slots[index].descriptor = None;
        self.slots[index].generation = next_generation;
        self.free.push(index as u32);
        Ok(())
    }

    pub fn sample_frame(
        &mut self,
        revision: u64,
        available_control_revision: u64,
        requests: &[PoseSampleRequest],
        asset_server: &AssetServer,
        residency: &ResidencyStore,
        provider: &dyn PoseSamplingProvider,
    ) -> Result<PresentationAnimationFrame, PresentationAnimationError> {
        self.ensure_open()?;
        if revision
            != self
                .revision
                .checked_add(1)
                .ok_or(PresentationAnimationError::RevisionSequence)?
        {
            return Err(PresentationAnimationError::RevisionSequence);
        }
        if requests.len() > self.config.max_requests_per_frame {
            return Err(PresentationAnimationError::RequestCapacity);
        }
        if asset_server.engine() != self.engine || residency.engine() != self.engine {
            return Err(PresentationAnimationError::WrongEngine);
        }
        if residency.endpoint_generation() != self.endpoint_generation {
            return Err(PresentationAnimationError::StaleAssetEndpoint);
        }
        if requests.iter().any(|request| request.visible) {
            if provider.engine() != self.engine
                || provider.endpoint_generation() != self.endpoint_generation
            {
                return Err(PresentationAnimationError::StaleProvider);
            }
            if provider.state() != PoseProviderState::Ready {
                return Err(PresentationAnimationError::ProviderUnavailable);
            }
        }
        let mut seen = BTreeSet::new();
        let mut palette_bytes = 0usize;
        let mut packets = Vec::with_capacity(requests.len());
        for request in requests {
            if !seen.insert(request.animator) {
                return Err(PresentationAnimationError::DuplicateAnimatorRequest);
            }
            if request.interpolation_denominator == 0
                || request.interpolation_numerator > request.interpolation_denominator
            {
                return Err(PresentationAnimationError::InvalidInterpolation);
            }
            if request.required_control_revision > available_control_revision {
                return Err(PresentationAnimationError::ControlBarrier);
            }
            let index = self.animator_index(request.animator)?;
            let descriptor = self.slots[index]
                .descriptor
                .as_ref()
                .ok_or(PresentationAnimationError::InvalidAnimator)?;
            if !request.visible {
                packets.push(PresentationPosePacket {
                    animator: request.animator,
                    entity: descriptor.entity,
                    palette: Vec::new(),
                    dispatch: None,
                    culled: true,
                });
                continue;
            }
            if !descriptor.assets.clips.contains(&request.previous_clip)
                || !descriptor.assets.clips.contains(&request.current_clip)
            {
                return Err(PresentationAnimationError::ClipNotBound);
            }
            let cpu_assets = std::iter::once(descriptor.assets.skeleton)
                .chain(std::iter::once(descriptor.assets.graph))
                .chain(std::iter::once(request.previous_clip))
                .chain(std::iter::once(request.current_clip))
                .chain(descriptor.assets.masks.iter().copied());
            if cpu_assets
                .into_iter()
                .any(|asset| asset_server.node_state(asset) != Some(CpuNodeState::CpuReady))
            {
                return Err(PresentationAnimationError::CpuAssetNotReady);
            }
            if residency.state(descriptor.assets.mesh, request.residency_key)
                != Some(ResidencyState::Ready)
                || residency.state(descriptor.assets.material, request.residency_key)
                    != Some(ResidencyState::Ready)
            {
                return Err(PresentationAnimationError::GpuAssetNotReady);
            }
            let previous = provider
                .sample_pose(
                    descriptor.assets.skeleton,
                    request.previous_clip,
                    request.previous_tick,
                    descriptor.bone_count,
                )
                .map_err(PresentationAnimationError::Provider)?;
            let current = provider
                .sample_pose(
                    descriptor.assets.skeleton,
                    request.current_clip,
                    request.current_tick,
                    descriptor.bone_count,
                )
                .map_err(PresentationAnimationError::Provider)?;
            if previous.len() != descriptor.bone_count as usize
                || current.len() != descriptor.bone_count as usize
            {
                return Err(PresentationAnimationError::ProviderContract);
            }
            let mut palette: Vec<_> = previous
                .iter()
                .zip(&current)
                .map(|(previous, current)| {
                    blend_transform(
                        *previous,
                        *current,
                        u64::from(request.interpolation_numerator),
                        u64::from(request.interpolation_denominator),
                    )
                })
                .collect::<Result<_, _>>()?;
            let layers = provider
                .evaluate_graph(descriptor.assets.graph, *request)
                .map_err(PresentationAnimationError::Provider)?;
            if layers.len() > self.config.max_graph_layers_per_animator {
                return Err(PresentationAnimationError::GraphCapacity);
            }
            for layer in layers {
                if layer.weight_denominator == 0
                    || layer.weight_numerator > layer.weight_denominator
                {
                    return Err(PresentationAnimationError::InvalidInterpolation);
                }
                if !descriptor.assets.clips.contains(&layer.clip) {
                    return Err(PresentationAnimationError::ClipNotBound);
                }
                if asset_server.node_state(layer.clip) != Some(CpuNodeState::CpuReady) {
                    return Err(PresentationAnimationError::CpuAssetNotReady);
                }
                if let Some(mask) = layer.mask {
                    if !descriptor.assets.masks.contains(&mask) {
                        return Err(PresentationAnimationError::MaskNotBound);
                    }
                    if asset_server.node_state(mask) != Some(CpuNodeState::CpuReady) {
                        return Err(PresentationAnimationError::CpuAssetNotReady);
                    }
                }
                let layer_pose = provider
                    .sample_pose(
                        descriptor.assets.skeleton,
                        layer.clip,
                        layer.tick,
                        descriptor.bone_count,
                    )
                    .map_err(PresentationAnimationError::Provider)?;
                if layer_pose.len() != descriptor.bone_count as usize {
                    return Err(PresentationAnimationError::ProviderContract);
                }
                let mask = if let Some(mask) = layer.mask {
                    let weights = provider
                        .sample_mask(mask, descriptor.bone_count)
                        .map_err(PresentationAnimationError::Provider)?;
                    if weights.len() != descriptor.bone_count as usize {
                        return Err(PresentationAnimationError::ProviderContract);
                    }
                    Some(weights)
                } else {
                    None
                };
                for (index, (bone, layer_bone)) in palette.iter_mut().zip(layer_pose).enumerate() {
                    let mask_weight = mask
                        .as_ref()
                        .map_or(u64::from(u16::MAX), |weights| u64::from(weights[index]));
                    let numerator = u64::from(layer.weight_numerator)
                        .checked_mul(mask_weight)
                        .ok_or(PresentationAnimationError::ArithmeticOverflow)?;
                    let denominator = u64::from(layer.weight_denominator)
                        .checked_mul(u64::from(u16::MAX))
                        .ok_or(PresentationAnimationError::ArithmeticOverflow)?;
                    *bone = blend_transform(*bone, layer_bone, numerator, denominator)?;
                }
            }
            let packet_bytes = palette
                .len()
                .checked_mul(std::mem::size_of::<BoneTransform>())
                .ok_or(PresentationAnimationError::PaletteCapacity)?;
            palette_bytes = palette_bytes
                .checked_add(packet_bytes)
                .ok_or(PresentationAnimationError::PaletteCapacity)?;
            if palette_bytes > self.config.max_palette_bytes_per_frame {
                return Err(PresentationAnimationError::PaletteCapacity);
            }
            packets.push(PresentationPosePacket {
                animator: request.animator,
                entity: descriptor.entity,
                palette,
                dispatch: Some(GpuSkinningDispatch {
                    skeleton: descriptor.assets.skeleton,
                    mesh: descriptor.assets.mesh,
                    material: descriptor.assets.material,
                    bone_count: descriptor.bone_count,
                    palette_bytes: packet_bytes,
                    residency_key: request.residency_key,
                }),
                culled: false,
            });
        }
        self.revision = revision;
        Ok(PresentationAnimationFrame { revision, packets })
    }

    fn animator_index(
        &self,
        animator: PresentationAnimatorId,
    ) -> Result<usize, PresentationAnimationError> {
        self.ensure_open()?;
        if animator.engine != self.engine {
            return Err(PresentationAnimationError::WrongEngine);
        }
        if animator.endpoint_generation != self.endpoint_generation {
            return Err(PresentationAnimationError::StaleEndpoint);
        }
        let index = animator.handle.index as usize;
        let slot = self
            .slots
            .get(index)
            .ok_or(PresentationAnimationError::InvalidAnimator)?;
        if animator.handle.generation == 0
            || animator.handle.generation != slot.generation
            || slot.descriptor.is_none()
        {
            return Err(PresentationAnimationError::InvalidAnimator);
        }
        Ok(index)
    }

    fn ensure_open(&self) -> Result<(), PresentationAnimationError> {
        if self.closed {
            Err(PresentationAnimationError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_descriptor(
    engine: EngineId,
    config: &PresentationAnimationConfig,
    descriptor: &PresentationAnimatorDescriptor,
) -> Result<(), PresentationAnimationError> {
    if descriptor.entity.world.engine != engine {
        return Err(PresentationAnimationError::WrongEngine);
    }
    if descriptor.bone_count == 0 {
        return Err(PresentationAnimationError::InvalidDescriptor);
    }
    if descriptor.bone_count as usize > config.max_bones_per_animator {
        return Err(PresentationAnimationError::BoneCapacity);
    }
    if descriptor.assets.clips.is_empty()
        || descriptor.assets.clips.len() > config.max_clips_per_animator
        || descriptor.assets.masks.len() > config.max_masks_per_animator
    {
        return Err(PresentationAnimationError::AssetCapacity);
    }
    let all_assets = std::iter::once(descriptor.assets.skeleton)
        .chain(std::iter::once(descriptor.assets.graph))
        .chain(descriptor.assets.clips.iter().copied())
        .chain(descriptor.assets.masks.iter().copied())
        .chain(std::iter::once(descriptor.assets.mesh))
        .chain(std::iter::once(descriptor.assets.material));
    if all_assets
        .into_iter()
        .any(|asset| asset.engine != engine || !asset.engine.is_valid() || !asset.handle.is_valid())
    {
        return Err(PresentationAnimationError::WrongEngine);
    }
    let clips: BTreeSet<_> = descriptor.assets.clips.iter().copied().collect();
    let masks: BTreeSet<_> = descriptor.assets.masks.iter().copied().collect();
    if clips.len() != descriptor.assets.clips.len() || masks.len() != descriptor.assets.masks.len()
    {
        return Err(PresentationAnimationError::InvalidDescriptor);
    }
    Ok(())
}

fn blend_transform(
    previous: BoneTransform,
    current: BoneTransform,
    numerator: u64,
    denominator: u64,
) -> Result<BoneTransform, PresentationAnimationError> {
    Ok(BoneTransform {
        translation: blend_array(
            previous.translation,
            current.translation,
            numerator,
            denominator,
        )?,
        rotation: blend_array(previous.rotation, current.rotation, numerator, denominator)?,
        scale: blend_array(previous.scale, current.scale, numerator, denominator)?,
    })
}

fn blend_array<const N: usize>(
    previous: [i64; N],
    current: [i64; N],
    numerator: u64,
    denominator: u64,
) -> Result<[i64; N], PresentationAnimationError> {
    let mut output = [0; N];
    let previous_weight = denominator - numerator;
    for index in 0..N {
        let value = i128::from(previous[index])
            .checked_mul(i128::from(previous_weight))
            .and_then(|value| {
                i128::from(current[index])
                    .checked_mul(i128::from(numerator))
                    .and_then(|current| value.checked_add(current))
            })
            .ok_or(PresentationAnimationError::ArithmeticOverflow)?
            / i128::from(denominator);
        output[index] =
            i64::try_from(value).map_err(|_| PresentationAnimationError::ArithmeticOverflow)?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::{
        asset::{
            ArtifactId, AssetId, AssetRef, AssetRegistration, AssetScopeKind, AssetServerConfig,
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

    fn descriptor(engine: EngineId) -> PresentationAnimatorDescriptor {
        PresentationAnimatorDescriptor {
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
            assets: AnimationAssetBinding {
                skeleton: asset(engine, 1),
                graph: asset(engine, 2),
                clips: vec![asset(engine, 3), asset(engine, 4)],
                masks: vec![asset(engine, 5)],
                mesh: asset(engine, 6),
                material: asset(engine, 7),
            },
            bone_count: 2,
        }
    }

    fn residency_key() -> ResidencyKey {
        ResidencyKey {
            device: handle(40),
            usage: 1,
            format: 2,
            quality: 3,
        }
    }

    fn ready_stores(engine: EngineId, render_generation: Handle) -> (AssetServer, ResidencyStore) {
        let mut assets =
            AssetServer::new(engine, handle(20), AssetServerConfig::default()).unwrap();
        let registrations: Vec<_> = (1u8..=7)
            .map(|index| AssetRegistration {
                asset_id: AssetId([index; 16]),
                asset_type: u64::from(index),
                source_revision: 1,
                artifact_id: ArtifactId([index + 10; 16]),
                dependencies: Vec::new(),
            })
            .collect();
        let refs = assets.register_batch(registrations).unwrap();
        let scope = assets.create_scope(AssetScopeKind::Engine).unwrap();
        for asset in refs {
            assets.request(scope, asset, 100).unwrap();
        }
        while let Some(work) = assets.poll_work() {
            assets.complete(&work).unwrap();
        }

        let mut residency = ResidencyStore::new(
            engine,
            render_generation,
            ResidencyConfig {
                max_records: 16,
                max_ready_bytes: 1024,
            },
        )
        .unwrap();
        for (asset, artifact) in [(asset(engine, 6), 16), (asset(engine, 7), 17)] {
            residency
                .request(asset, ArtifactId([artifact; 16]), residency_key(), 64)
                .unwrap();
            let work = residency.poll_upload().unwrap();
            residency.complete_upload(work, true).unwrap();
        }
        (assets, residency)
    }

    struct Provider {
        engine: EngineId,
        endpoint_generation: Handle,
        state: PoseProviderState,
        calls: Cell<usize>,
        wrong_count: bool,
    }

    impl PoseSamplingProvider for Provider {
        fn engine(&self) -> EngineId {
            self.engine
        }

        fn endpoint_generation(&self) -> Handle {
            self.endpoint_generation
        }

        fn state(&self) -> PoseProviderState {
            self.state
        }

        fn sample_pose(
            &self,
            _skeleton: AssetRef,
            _clip: AssetRef,
            tick: u64,
            bone_count: u32,
        ) -> Result<Vec<BoneTransform>, PoseProviderError> {
            self.calls.set(self.calls.get() + 1);
            let count = if self.wrong_count { 1 } else { bone_count };
            Ok((0..count)
                .map(|bone| BoneTransform {
                    translation: [tick as i64, bone as i64, 0],
                    rotation: [0, 0, 0, 1_000],
                    scale: [1_000; 3],
                })
                .collect())
        }
    }

    struct GraphProvider {
        engine: EngineId,
        endpoint_generation: Handle,
        layers: Vec<AnimationBlendLayer>,
        mask_weights: Vec<u16>,
        pose_calls: Cell<usize>,
    }

    impl PoseSamplingProvider for GraphProvider {
        fn engine(&self) -> EngineId {
            self.engine
        }

        fn endpoint_generation(&self) -> Handle {
            self.endpoint_generation
        }

        fn state(&self) -> PoseProviderState {
            PoseProviderState::Ready
        }

        fn evaluate_graph(
            &self,
            _graph: AssetRef,
            _request: PoseSampleRequest,
        ) -> Result<Vec<AnimationBlendLayer>, PoseProviderError> {
            Ok(self.layers.clone())
        }

        fn sample_pose(
            &self,
            _skeleton: AssetRef,
            _clip: AssetRef,
            tick: u64,
            bone_count: u32,
        ) -> Result<Vec<BoneTransform>, PoseProviderError> {
            self.pose_calls.set(self.pose_calls.get() + 1);
            Ok((0..bone_count)
                .map(|bone| BoneTransform {
                    translation: [tick as i64, bone as i64, 0],
                    rotation: [0, 0, 0, 1_000],
                    scale: [1_000; 3],
                })
                .collect())
        }

        fn sample_mask(
            &self,
            _mask: AssetRef,
            _bone_count: u32,
        ) -> Result<Vec<u16>, PoseProviderError> {
            Ok(self.mask_weights.clone())
        }
    }

    #[test]
    fn samples_blends_and_builds_palette_dispatch_with_exact_assets() {
        let engine = handle(1);
        let generation = handle(9);
        let descriptor = descriptor(engine);
        let mut endpoint = PresentationAnimationEndpoint::new(
            engine,
            generation,
            PresentationAnimationConfig::default(),
        )
        .unwrap();
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let (assets, residency) = ready_stores(engine, generation);
        let provider = Provider {
            engine,
            endpoint_generation: generation,
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: false,
        };
        let frame = endpoint
            .sample_frame(
                1,
                1,
                &[PoseSampleRequest {
                    animator,
                    previous_clip: descriptor.assets.clips[0],
                    previous_tick: 10,
                    current_clip: descriptor.assets.clips[1],
                    current_tick: 14,
                    interpolation_numerator: 1,
                    interpolation_denominator: 2,
                    required_control_revision: 1,
                    residency_key: residency_key(),
                    visible: true,
                }],
                &assets,
                &residency,
                &provider,
            )
            .unwrap();
        assert_eq!(provider.calls.get(), 2);
        assert_eq!(frame.packets[0].palette[0].translation[0], 12);
        let dispatch = frame.packets[0].dispatch.unwrap();
        assert_eq!(dispatch.mesh, descriptor.assets.mesh);
        assert_eq!(dispatch.material, descriptor.assets.material);
        assert_eq!(dispatch.bone_count, 2);
    }

    #[test]
    fn culled_animator_skips_sampling_and_palette_allocation() {
        let engine = handle(1);
        let mut endpoint = PresentationAnimationEndpoint::new(
            engine,
            handle(9),
            PresentationAnimationConfig::default(),
        )
        .unwrap();
        let descriptor = descriptor(engine);
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let (mut assets, mut residency) = ready_stores(engine, handle(9));
        assets
            .hot_reload(AssetRegistration {
                asset_id: AssetId([2; 16]),
                asset_type: 2,
                source_revision: 2,
                artifact_id: ArtifactId([99; 16]),
                dependencies: Vec::new(),
            })
            .unwrap();
        residency.device_lost(residency_key().device);
        let provider = Provider {
            engine,
            endpoint_generation: handle(9),
            state: PoseProviderState::Closing,
            calls: Cell::new(0),
            wrong_count: false,
        };
        let frame = endpoint
            .sample_frame(
                1,
                1,
                &[PoseSampleRequest {
                    animator,
                    previous_clip: descriptor.assets.clips[0],
                    previous_tick: 0,
                    current_clip: descriptor.assets.clips[1],
                    current_tick: 1,
                    interpolation_numerator: 1,
                    interpolation_denominator: 2,
                    required_control_revision: 1,
                    residency_key: residency_key(),
                    visible: false,
                }],
                &assets,
                &residency,
                &provider,
            )
            .unwrap();
        assert_eq!(provider.calls.get(), 0);
        assert!(frame.packets[0].culled);
        assert!(frame.packets[0].palette.is_empty());
        assert_eq!(frame.packets[0].dispatch, None);
    }

    #[test]
    fn provider_contract_and_palette_budget_fail_without_advancing_revision() {
        let engine = handle(1);
        let mut config = PresentationAnimationConfig::default();
        config.max_palette_bytes_per_frame = std::mem::size_of::<BoneTransform>();
        let mut endpoint = PresentationAnimationEndpoint::new(engine, handle(9), config).unwrap();
        let descriptor = descriptor(engine);
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let (assets, residency) = ready_stores(engine, handle(9));
        let request = PoseSampleRequest {
            animator,
            previous_clip: descriptor.assets.clips[0],
            previous_tick: 0,
            current_clip: descriptor.assets.clips[1],
            current_tick: 1,
            interpolation_numerator: 1,
            interpolation_denominator: 2,
            required_control_revision: 1,
            residency_key: residency_key(),
            visible: true,
        };
        let wrong = Provider {
            engine,
            endpoint_generation: handle(9),
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: true,
        };
        assert_eq!(
            endpoint.sample_frame(1, 1, &[request], &assets, &residency, &wrong),
            Err(PresentationAnimationError::ProviderContract)
        );
        assert_eq!(endpoint.revision(), 0);
        let provider = Provider {
            engine,
            endpoint_generation: handle(9),
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: false,
        };
        assert_eq!(
            endpoint.sample_frame(1, 1, &[request], &assets, &residency, &provider),
            Err(PresentationAnimationError::PaletteCapacity)
        );
        assert_eq!(endpoint.revision(), 0);
    }

    #[test]
    fn engine_endpoint_and_generation_are_checked_before_sampling() {
        let engine = handle(1);
        let generation = handle(9);
        let mut endpoint = PresentationAnimationEndpoint::new(
            engine,
            generation,
            PresentationAnimationConfig::default(),
        )
        .unwrap();
        let descriptor = descriptor(engine);
        let first = endpoint.create_animator(descriptor.clone()).unwrap();
        endpoint.destroy_animator(first).unwrap();
        let current = endpoint.create_animator(descriptor.clone()).unwrap();
        let (assets, residency) = ready_stores(engine, generation);
        let provider = Provider {
            engine,
            endpoint_generation: generation,
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: false,
        };
        let mut request = PoseSampleRequest {
            animator: first,
            previous_clip: descriptor.assets.clips[0],
            previous_tick: 0,
            current_clip: descriptor.assets.clips[1],
            current_tick: 1,
            interpolation_numerator: 0,
            interpolation_denominator: 1,
            required_control_revision: 1,
            residency_key: residency_key(),
            visible: true,
        };
        assert_eq!(
            endpoint.sample_frame(1, 1, &[request], &assets, &residency, &provider),
            Err(PresentationAnimationError::InvalidAnimator)
        );
        request.animator = PresentationAnimatorId {
            endpoint_generation: handle(10),
            ..current
        };
        assert_eq!(
            endpoint.sample_frame(1, 1, &[request], &assets, &residency, &provider),
            Err(PresentationAnimationError::StaleEndpoint)
        );
        assert_eq!(provider.calls.get(), 0);
        assert_eq!(endpoint.revision(), 0);
    }

    #[test]
    fn control_asset_residency_and_provider_barriers_precede_sampling() {
        let engine = handle(1);
        let generation = handle(9);
        let mut endpoint = PresentationAnimationEndpoint::new(
            engine,
            generation,
            PresentationAnimationConfig::default(),
        )
        .unwrap();
        let descriptor = descriptor(engine);
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let request = PoseSampleRequest {
            animator,
            previous_clip: descriptor.assets.clips[0],
            previous_tick: 0,
            current_clip: descriptor.assets.clips[1],
            current_tick: 1,
            interpolation_numerator: 1,
            interpolation_denominator: 2,
            required_control_revision: 5,
            residency_key: residency_key(),
            visible: true,
        };
        let provider = Provider {
            engine,
            endpoint_generation: generation,
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: false,
        };
        let (mut assets, mut residency) = ready_stores(engine, generation);
        assert_eq!(
            endpoint.sample_frame(1, 4, &[request], &assets, &residency, &provider),
            Err(PresentationAnimationError::ControlBarrier)
        );

        assets
            .hot_reload(AssetRegistration {
                asset_id: AssetId([2; 16]),
                asset_type: 2,
                source_revision: 2,
                artifact_id: ArtifactId([99; 16]),
                dependencies: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            endpoint.sample_frame(1, 5, &[request], &assets, &residency, &provider),
            Err(PresentationAnimationError::CpuAssetNotReady)
        );

        let (ready_assets, _) = ready_stores(engine, generation);
        residency.device_lost(residency_key().device);
        assert_eq!(
            endpoint.sample_frame(1, 5, &[request], &ready_assets, &residency, &provider,),
            Err(PresentationAnimationError::GpuAssetNotReady)
        );

        let (_, ready_residency) = ready_stores(engine, generation);
        let closing = Provider {
            engine,
            endpoint_generation: generation,
            state: PoseProviderState::Closing,
            calls: Cell::new(0),
            wrong_count: false,
        };
        assert_eq!(
            endpoint.sample_frame(1, 5, &[request], &ready_assets, &ready_residency, &closing,),
            Err(PresentationAnimationError::ProviderUnavailable)
        );
        let restarted = Provider {
            engine,
            endpoint_generation: handle(10),
            state: PoseProviderState::Ready,
            calls: Cell::new(0),
            wrong_count: false,
        };
        assert_eq!(
            endpoint.sample_frame(
                1,
                5,
                &[request],
                &ready_assets,
                &ready_residency,
                &restarted,
            ),
            Err(PresentationAnimationError::StaleProvider)
        );
        assert_eq!(provider.calls.get(), 0);
        assert_eq!(endpoint.revision(), 0);
    }

    #[test]
    fn graph_layer_uses_bound_mask_weights_per_bone_in_stable_order() {
        let engine = handle(1);
        let generation = handle(9);
        let mut endpoint = PresentationAnimationEndpoint::new(
            engine,
            generation,
            PresentationAnimationConfig::default(),
        )
        .unwrap();
        let descriptor = descriptor(engine);
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let (assets, residency) = ready_stores(engine, generation);
        let provider = GraphProvider {
            engine,
            endpoint_generation: generation,
            layers: vec![AnimationBlendLayer {
                clip: descriptor.assets.clips[1],
                tick: 10,
                weight_numerator: 1,
                weight_denominator: 1,
                mask: Some(descriptor.assets.masks[0]),
            }],
            mask_weights: vec![0, u16::MAX],
            pose_calls: Cell::new(0),
        };
        let frame = endpoint
            .sample_frame(
                1,
                0,
                &[PoseSampleRequest {
                    animator,
                    previous_clip: descriptor.assets.clips[0],
                    previous_tick: 0,
                    current_clip: descriptor.assets.clips[0],
                    current_tick: 0,
                    interpolation_numerator: 0,
                    interpolation_denominator: 1,
                    required_control_revision: 0,
                    residency_key: residency_key(),
                    visible: true,
                }],
                &assets,
                &residency,
                &provider,
            )
            .unwrap();
        assert_eq!(provider.pose_calls.get(), 3);
        assert_eq!(frame.packets[0].palette[0].translation[0], 0);
        assert_eq!(frame.packets[0].palette[1].translation[0], 10);
    }

    #[test]
    fn graph_capacity_and_unbound_dependencies_leave_revision_unchanged() {
        let engine = handle(1);
        let generation = handle(9);
        let mut config = PresentationAnimationConfig::default();
        config.max_graph_layers_per_animator = 1;
        let mut endpoint = PresentationAnimationEndpoint::new(engine, generation, config).unwrap();
        let descriptor = descriptor(engine);
        let animator = endpoint.create_animator(descriptor.clone()).unwrap();
        let (assets, residency) = ready_stores(engine, generation);
        let request = PoseSampleRequest {
            animator,
            previous_clip: descriptor.assets.clips[0],
            previous_tick: 0,
            current_clip: descriptor.assets.clips[0],
            current_tick: 0,
            interpolation_numerator: 0,
            interpolation_denominator: 1,
            required_control_revision: 0,
            residency_key: residency_key(),
            visible: true,
        };
        let layer = AnimationBlendLayer {
            clip: descriptor.assets.clips[1],
            tick: 1,
            weight_numerator: 1,
            weight_denominator: 1,
            mask: None,
        };
        let too_many = GraphProvider {
            engine,
            endpoint_generation: generation,
            layers: vec![layer, layer],
            mask_weights: Vec::new(),
            pose_calls: Cell::new(0),
        };
        assert_eq!(
            endpoint.sample_frame(1, 0, &[request], &assets, &residency, &too_many),
            Err(PresentationAnimationError::GraphCapacity)
        );
        let unbound = GraphProvider {
            layers: vec![AnimationBlendLayer {
                clip: asset(engine, 99),
                ..layer
            }],
            ..too_many
        };
        assert_eq!(
            endpoint.sample_frame(1, 0, &[request], &assets, &residency, &unbound),
            Err(PresentationAnimationError::ClipNotBound)
        );
        assert_eq!(endpoint.revision(), 0);
    }
}
