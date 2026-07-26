use std::collections::BTreeMap;

use voplay_render_2d::{ReferenceDrawResolver2d, Scene2dFrameEncoder, Scene2dFrameEncoderConfig};
use voplay_render_3d::{
    decode_scene_frame_3d, encode_scene_frame_3d, is_scene_frame_3d, RenderFeatureFactoryBuilder,
    Scene3dFrameEncoder, Scene3dFrameEncoderConfig, SceneFrame3d,
};
use voplay_runtime::{
    presentation::{FrameCommandEncoder, FrameEncodeRequest, FrameEncoderError, RenderObjectView},
    render_commands::{
        decode_frame_commands, decode_object_commands, decode_target_frame_bundle,
        encode_frame_commands, encode_target_frame_bundle, is_target_frame_bundle,
        TargetDrawCommands,
    },
    RenderEntity,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObjectProfile {
    TwoD,
    ThreeD,
}

pub struct FullFrameEncoder {
    two_d: Scene2dFrameEncoder<ReferenceDrawResolver2d>,
    three_d: Scene3dFrameEncoder,
    objects: BTreeMap<RenderEntity, ObjectProfile>,
    max_commands: usize,
    max_encoded_bytes: usize,
    closed: bool,
}

impl FullFrameEncoder {
    pub fn new_with_render_feature_factories(
        engine: voplay_protocol::EngineId,
        factories: &[RenderFeatureFactoryBuilder],
    ) -> Result<Self, FrameEncoderError> {
        let two_d_config = Scene2dFrameEncoderConfig {
            allow_missing_views: true,
            ..Scene2dFrameEncoderConfig::default()
        };
        let three_d_config = Scene3dFrameEncoderConfig {
            allow_missing_cameras: true,
            ..Scene3dFrameEncoderConfig::default()
        };
        let mut three_d = Scene3dFrameEncoder::new(engine, three_d_config)?;
        for factory in factories {
            three_d.register_render_feature_factory(*factory)?;
        }
        Ok(Self {
            two_d: Scene2dFrameEncoder::new(two_d_config, ReferenceDrawResolver2d)?,
            three_d,
            objects: BTreeMap::new(),
            max_commands: 1_000_000,
            max_encoded_bytes: 64 * 1024 * 1024,
            closed: false,
        })
    }
}

impl FrameCommandEncoder for FullFrameEncoder {
    fn owner_snapshot(&self) -> voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
        let two_d = self.two_d.owner_snapshot();
        let three_d = self.three_d.owner_snapshot();
        voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
            closed: self.closed,
            retained_objects: self
                .objects
                .len()
                .saturating_add(two_d.retained_objects)
                .saturating_add(three_d.retained_objects),
            retained_assets: two_d
                .retained_assets
                .saturating_add(three_d.retained_assets),
            retained_bytes: two_d.retained_bytes.saturating_add(three_d.retained_bytes),
        }
    }

    fn shutdown(&mut self) {
        self.closed = true;
        self.two_d.shutdown();
        self.three_d.shutdown();
        self.objects.clear();
    }

    fn register_render_feature(&mut self, bytes: &[u8]) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        self.three_d.register_render_feature(bytes)
    }

    fn upsert_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        self.three_d.upsert_asset(kind, asset, revision, bytes)
    }

    fn remove_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        self.three_d.remove_asset(kind, asset, revision)
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        let mut objects = if request.full_snapshot {
            BTreeMap::new()
        } else {
            self.objects.clone()
        };
        let mut two_d_objects = Vec::new();
        let mut three_d_objects = Vec::new();
        let mut two_d_despawned = Vec::new();
        let mut three_d_despawned = Vec::new();

        for entity in request.despawned {
            match objects.remove(entity) {
                Some(ObjectProfile::TwoD) => two_d_despawned.push(*entity),
                Some(ObjectProfile::ThreeD) => three_d_despawned.push(*entity),
                None => {}
            }
        }
        for object in &request.objects {
            let profile = classify(object.bytes)?;
            if let Some(previous) = objects.insert(object.entity, profile) {
                if previous != profile {
                    match previous {
                        ObjectProfile::TwoD => two_d_despawned.push(object.entity),
                        ObjectProfile::ThreeD => three_d_despawned.push(object.entity),
                    }
                }
            }
            match profile {
                ObjectProfile::TwoD => two_d_objects.push(RenderObjectView {
                    entity: object.entity,
                    bytes: object.bytes,
                }),
                ObjectProfile::ThreeD => three_d_objects.push(RenderObjectView {
                    entity: object.entity,
                    bytes: object.bytes,
                }),
            }
        }

        let mut two_d = self.two_d.clone();
        let mut three_d = self.three_d.clone();
        let two_d_frame = two_d.encode(FrameEncodeRequest {
            engine: request.engine,
            domain: request.domain,
            pulse_id: request.pulse_id,
            render_revision: request.render_revision,
            control_revision: request.control_revision,
            graph_signature: request.graph_signature,
            control_frame: request.control_frame,
            full_snapshot: request.full_snapshot,
            objects: two_d_objects,
            despawned: &two_d_despawned,
            transient: None,
        })?;
        let three_d_frame = three_d.encode(FrameEncodeRequest {
            engine: request.engine,
            domain: request.domain,
            pulse_id: request.pulse_id,
            render_revision: request.render_revision,
            control_revision: request.control_revision,
            graph_signature: request.graph_signature,
            control_frame: request.control_frame,
            full_snapshot: request.full_snapshot,
            objects: three_d_objects,
            despawned: &three_d_despawned,
            transient: None,
        })?;
        let mut targets =
            BTreeMap::<voplay_runtime::control::StableControlRef, TargetDrawCommands>::new();
        let mut native_overlay_targets =
            BTreeMap::<voplay_runtime::control::StableControlRef, TargetDrawCommands>::new();
        let max_targets = request
            .control_frame
            .map_or(1, |plan| plan.targets.len().max(1));
        let scene_frame = if is_scene_frame_3d(&three_d_frame) {
            Some(
                decode_scene_frame_3d(
                    &three_d_frame,
                    request.engine,
                    max_targets,
                    self.max_commands,
                    16_384,
                    self.max_encoded_bytes,
                )
                .map_err(|_| FrameEncoderError::InvalidInput)?,
            )
        } else {
            None
        };
        merge_frame(
            &mut targets,
            scene_frame
                .as_ref()
                .map_or(three_d_frame.as_slice(), |frame| {
                    frame.fallback_commands.as_slice()
                }),
            request.engine,
            max_targets,
            self.max_commands,
            self.max_encoded_bytes,
        )?;
        merge_frame(
            &mut targets,
            &two_d_frame,
            request.engine,
            max_targets,
            self.max_commands,
            self.max_encoded_bytes,
        )?;
        merge_frame(
            &mut native_overlay_targets,
            &two_d_frame,
            request.engine,
            max_targets,
            self.max_commands,
            self.max_encoded_bytes,
        )?;
        if let Some(transient) = request.transient {
            let target = targets
                .values()
                .filter(|target| target.external_surface)
                .map(|target| target.target)
                .min()
                .or_else(|| targets.keys().next().copied())
                .ok_or(FrameEncoderError::InvalidInput)?;
            let transient_commands = decode_object_commands(&transient.bytes)?;
            targets
                .get_mut(&target)
                .unwrap()
                .commands
                .extend(transient_commands.iter().copied());
            let target_descriptor = targets[&target].clone();
            native_overlay_targets
                .entry(target)
                .or_insert_with(|| TargetDrawCommands {
                    commands: Vec::new(),
                    ..target_descriptor
                })
                .commands
                .extend(transient_commands);
        }
        if targets
            .values()
            .map(|target| target.commands.len())
            .sum::<usize>()
            > self.max_commands
        {
            return Err(FrameEncoderError::Capacity);
        }
        let portable_encoded = if targets.is_empty() {
            encode_frame_commands(&[], self.max_encoded_bytes)?
        } else {
            encode_target_frame_bundle(
                &targets.into_values().collect::<Vec<_>>(),
                max_targets,
                self.max_commands,
                self.max_encoded_bytes,
            )?
        };
        let native_overlay_encoded = if native_overlay_targets.is_empty() {
            encode_frame_commands(&[], self.max_encoded_bytes)?
        } else {
            encode_target_frame_bundle(
                &native_overlay_targets.into_values().collect::<Vec<_>>(),
                max_targets,
                self.max_commands,
                self.max_encoded_bytes,
            )?
        };
        let encoded = if let Some(scene) = scene_frame {
            encode_scene_frame_3d(
                &SceneFrame3d {
                    fallback_commands: portable_encoded,
                    native_overlay_commands: native_overlay_encoded,
                    dynamic_meshes: scene.dynamic_meshes,
                    views: scene.views,
                },
                max_targets,
                self.max_commands,
                16_384,
                self.max_encoded_bytes,
            )
            .map_err(|_| FrameEncoderError::Capacity)?
        } else {
            portable_encoded
        };
        self.two_d = two_d;
        self.three_d = three_d;
        self.objects = objects;
        Ok(encoded)
    }
}

fn merge_frame(
    targets: &mut BTreeMap<voplay_runtime::control::StableControlRef, TargetDrawCommands>,
    bytes: &[u8],
    engine: voplay_protocol::EngineId,
    max_targets: usize,
    max_commands: usize,
    max_bytes: usize,
) -> Result<(), FrameEncoderError> {
    if !is_target_frame_bundle(bytes) {
        let commands = decode_frame_commands(bytes)?;
        if commands.is_empty() {
            return Ok(());
        }
        return Err(FrameEncoderError::InvalidInput);
    }
    for target in decode_target_frame_bundle(bytes, engine, max_targets, max_commands, max_bytes)? {
        match targets.entry(target.target) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(target);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get_mut();
                if current.width != target.width
                    || current.height != target.height
                    || current.external_surface != target.external_surface
                {
                    return Err(FrameEncoderError::InvalidInput);
                }
                current.commands.extend(target.commands);
            }
        }
    }
    Ok(())
}

fn classify(bytes: &[u8]) -> Result<ObjectProfile, FrameEncoderError> {
    match bytes.get(..4) {
        Some(b"VRW1") => Ok(ObjectProfile::ThreeD),
        Some(b"VV21" | b"VS21" | b"VH21" | b"VT21" | b"VC21" | b"VP21") => Ok(ObjectProfile::TwoD),
        _ => Err(FrameEncoderError::Unsupported),
    }
}
