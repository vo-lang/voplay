use std::collections::BTreeMap;

use voplay_runtime::{
    presentation::{FrameCommandEncoder, FrameEncodeRequest, FrameEncoderError},
    render_commands::{
        decode_object_commands, encode_frame_commands, encode_target_frame_bundle,
        TargetDrawCommands,
    },
    RenderEntity,
};

use crate::{
    decode_particle_emitter_2d, decode_shape_2d, decode_sprite_2d, decode_text_2d,
    decode_tile_chunk_2d, decode_view_2d, PortableDrawResolver2d, PortableScene2d,
    PortableScene2dConfig, PortableScene2dError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scene2dFrameEncoderConfig {
    pub scene: PortableScene2dConfig,
    pub max_commands: usize,
    pub max_encoded_bytes: usize,
    pub allow_missing_views: bool,
}

impl Default for Scene2dFrameEncoderConfig {
    fn default() -> Self {
        Self {
            scene: PortableScene2dConfig::default(),
            max_commands: 1_000_000,
            max_encoded_bytes: 64 * 1024 * 1024,
            allow_missing_views: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SceneObject2d {
    View(u64),
    Sprite(u64),
    Shape(u64),
    Text(u64),
    TileChunk(u64),
    ParticleEmitter(u64),
}

#[derive(Clone)]
pub struct Scene2dFrameEncoder<R> {
    config: Scene2dFrameEncoderConfig,
    scene: PortableScene2d,
    objects: BTreeMap<RenderEntity, SceneObject2d>,
    resolver: R,
    closed: bool,
}

impl<R: PortableDrawResolver2d + Clone> Scene2dFrameEncoder<R> {
    pub fn new(config: Scene2dFrameEncoderConfig, resolver: R) -> Result<Self, FrameEncoderError> {
        if config.max_commands == 0 || config.max_encoded_bytes < 8 {
            return Err(FrameEncoderError::InvalidInput);
        }
        Ok(Self {
            scene: PortableScene2d::new(config.scene).map_err(map_scene_error)?,
            config,
            objects: BTreeMap::new(),
            resolver,
            closed: false,
        })
    }

    pub const fn scene(&self) -> &PortableScene2d {
        &self.scene
    }

    pub const fn resolver(&self) -> &R {
        &self.resolver
    }

    pub fn resolver_mut(&mut self) -> &mut R {
        &mut self.resolver
    }

    pub fn into_resolver(self) -> R {
        self.resolver
    }
}

impl<R: PortableDrawResolver2d + Clone> FrameCommandEncoder for Scene2dFrameEncoder<R> {
    fn owner_snapshot(&self) -> voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
        let scene = self.scene.owner_snapshot();
        voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
            closed: self.closed,
            retained_objects: self.objects.len(),
            retained_assets: 0,
            retained_bytes: scene.text_bytes,
        }
    }

    fn shutdown(&mut self) {
        self.closed = true;
        self.objects.clear();
        self.scene.shutdown();
        self.resolver.shutdown();
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        if !request.engine.is_valid()
            || request.domain.engine != request.engine
            || request.pulse_id == 0
            || request.graph_signature == 0
        {
            return Err(FrameEncoderError::InvalidInput);
        }

        let mut scene = if request.full_snapshot {
            PortableScene2d::new(self.config.scene).map_err(map_scene_error)?
        } else {
            self.scene.clone()
        };
        let mut objects = if request.full_snapshot {
            BTreeMap::new()
        } else {
            self.objects.clone()
        };

        for entity in request.despawned {
            let object = objects
                .remove(entity)
                .ok_or(FrameEncoderError::InvalidInput)?;
            remove_object(&mut scene, object)?;
        }
        for object in request.objects {
            if object.entity.engine != request.engine {
                return Err(FrameEncoderError::InvalidInput);
            }
            let next = apply_object(&mut scene, object.bytes, &self.config.scene)?;
            if objects
                .iter()
                .any(|(entity, current)| *entity != object.entity && *current == next)
            {
                return Err(FrameEncoderError::InvalidInput);
            }
            if let Some(previous) = objects.insert(object.entity, next) {
                if previous != next {
                    remove_object(&mut scene, previous)?;
                }
            }
        }

        let mut resolver = self.resolver.clone();
        let control_frame = request.control_frame;
        let views = match control_frame {
            Some(plan) => {
                if plan.engine != request.engine || plan.domain != request.domain {
                    return Err(FrameEncoderError::InvalidInput);
                }
                plan.views
                    .iter()
                    .map(|view| {
                        (
                            presentation_view_id(
                                view.view.handle.index,
                                view.view.handle.generation,
                            ),
                            view.target,
                        )
                    })
                    .collect::<Vec<_>>()
            }
            None => vec![(
                presentation_view_id(
                    request.domain.handle.index,
                    request.domain.handle.generation,
                ),
                voplay_runtime::control::StableControlRef {
                    engine: request.engine,
                    kind: voplay_runtime::control::ControlKind::RenderTarget,
                    handle: request.domain.handle,
                },
            )],
        };
        if views.is_empty() {
            return Err(FrameEncoderError::InvalidInput);
        }
        let mut commands = Vec::new();
        let mut target_commands =
            BTreeMap::<voplay_runtime::control::StableControlRef, Vec<_>>::new();
        let mut encoded_views = 0_usize;
        for (view_id, expected_target) in views {
            let Some(view) = scene.view(view_id) else {
                if self.config.allow_missing_views {
                    continue;
                }
                return Err(FrameEncoderError::InvalidInput);
            };
            encoded_views += 1;
            if control_frame.is_some()
                && view.target
                    != presentation_view_id(
                        expected_target.handle.index,
                        expected_target.handle.generation,
                    )
            {
                return Err(FrameEncoderError::InvalidInput);
            }
            let mut view_commands = scene
                .portable_draw_commands(view_id, self.config.max_commands, &mut resolver)
                .map_err(map_scene_error)?;
            if let Some(voplay_runtime::render_commands::DrawCommand::Clear { color }) =
                view_commands.first().copied()
            {
                view_commands[0] = voplay_runtime::render_commands::DrawCommand::SolidRect {
                    x: view.viewport[0],
                    y: view.viewport[1],
                    width: view.viewport[2],
                    height: view.viewport[3],
                    color,
                };
            }
            if control_frame.is_some() {
                target_commands
                    .entry(expected_target)
                    .or_default()
                    .extend(view_commands);
            } else {
                commands.extend(view_commands);
            }
            if commands.len() > self.config.max_commands {
                return Err(FrameEncoderError::Capacity);
            }
        }
        if encoded_views == 0 && !self.config.allow_missing_views {
            return Err(FrameEncoderError::InvalidInput);
        }
        if let Some(transient) = request.transient {
            let transient = decode_object_commands(&transient.bytes)?;
            if let Some(plan) = control_frame {
                let target = plan
                    .targets
                    .iter()
                    .filter(|target| target.external_surface)
                    .map(|target| target.target)
                    .min()
                    .or_else(|| plan.targets.first().map(|target| target.target))
                    .ok_or(FrameEncoderError::InvalidInput)?;
                target_commands.entry(target).or_default().extend(transient);
            } else {
                commands.extend(transient);
            }
        }
        let total_commands = commands.len() + target_commands.values().map(Vec::len).sum::<usize>();
        if total_commands > self.config.max_commands {
            return Err(FrameEncoderError::Capacity);
        }
        let encoded = if let Some(plan) = control_frame {
            if target_commands.is_empty() {
                encode_frame_commands(&[], self.config.max_encoded_bytes)?
            } else {
                let targets = target_commands
                    .into_iter()
                    .map(|(target, commands)| {
                        let resolved = plan
                            .targets
                            .iter()
                            .find(|resolved| resolved.target == target)
                            .ok_or(FrameEncoderError::InvalidInput)?;
                        Ok(TargetDrawCommands {
                            target,
                            width: resolved.width,
                            height: resolved.height,
                            external_surface: resolved.external_surface,
                            color_format: resolved.color_format,
                            depth_format: resolved.depth_format,
                            sample_count: resolved.sample_count,
                            usage: resolved.usage,
                            commands,
                        })
                    })
                    .collect::<Result<Vec<_>, FrameEncoderError>>()?;
                encode_target_frame_bundle(
                    &targets,
                    plan.targets.len().max(1),
                    self.config.max_commands,
                    self.config.max_encoded_bytes,
                )?
            }
        } else {
            encode_frame_commands(&commands, self.config.max_encoded_bytes)?
        };
        self.scene = scene;
        self.objects = objects;
        self.resolver = resolver;
        Ok(encoded)
    }
}

pub const fn presentation_view_id(index: u32, generation: u32) -> u64 {
    (index as u64) | ((generation as u64) << 32)
}

fn apply_object(
    scene: &mut PortableScene2d,
    bytes: &[u8],
    config: &PortableScene2dConfig,
) -> Result<SceneObject2d, FrameEncoderError> {
    let magic = bytes.get(..4).ok_or(FrameEncoderError::InvalidInput)?;
    match magic {
        b"VV21" => {
            let value =
                decode_view_2d(bytes, config.max_post_effects_per_view).map_err(map_wire_error)?;
            scene.apply_view(bytes).map_err(map_scene_error)?;
            Ok(SceneObject2d::View(value.id))
        }
        b"VS21" => {
            let value = decode_sprite_2d(bytes).map_err(map_wire_error)?;
            scene.apply_sprite(bytes).map_err(map_scene_error)?;
            Ok(SceneObject2d::Sprite(value.id))
        }
        b"VH21" => {
            let value = decode_shape_2d(bytes, config.max_path_values).map_err(map_wire_error)?;
            scene.apply_shape(bytes).map_err(map_scene_error)?;
            Ok(SceneObject2d::Shape(value.id))
        }
        b"VT21" => {
            let value = decode_text_2d(bytes, config.max_text_bytes).map_err(map_wire_error)?;
            scene.apply_text(bytes).map_err(map_scene_error)?;
            Ok(SceneObject2d::Text(value.id))
        }
        b"VC21" => {
            let value = decode_tile_chunk_2d(bytes, config.max_tile_cells, config.max_tile_cells)
                .map_err(map_wire_error)?;
            scene.apply_tile_chunk(bytes).map_err(map_scene_error)?;
            Ok(SceneObject2d::TileChunk(value.chunk))
        }
        b"VP21" => {
            let value = decode_particle_emitter_2d(bytes).map_err(map_wire_error)?;
            scene
                .apply_particle_emitter(bytes)
                .map_err(map_scene_error)?;
            Ok(SceneObject2d::ParticleEmitter(value.id))
        }
        _ => Err(FrameEncoderError::Unsupported),
    }
}

fn remove_object(
    scene: &mut PortableScene2d,
    object: SceneObject2d,
) -> Result<(), FrameEncoderError> {
    let result = match object {
        SceneObject2d::View(id) => scene.remove_view(id),
        SceneObject2d::Sprite(id) => scene.remove_sprite(id),
        SceneObject2d::Shape(id) => scene.remove_shape(id),
        SceneObject2d::Text(id) => scene.remove_text(id),
        SceneObject2d::TileChunk(id) => scene.remove_tile_chunk(id),
        SceneObject2d::ParticleEmitter(id) => scene.remove_particle_emitter(id),
    };
    result.map(|_| ()).map_err(map_scene_error)
}

fn map_scene_error(error: PortableScene2dError) -> FrameEncoderError {
    match error {
        PortableScene2dError::Closed => FrameEncoderError::Closed,
        PortableScene2dError::Capacity => FrameEncoderError::Capacity,
        PortableScene2dError::InvalidConfig
        | PortableScene2dError::InvalidDescriptor
        | PortableScene2dError::StaleRevision
        | PortableScene2dError::ArithmeticOverflow
        | PortableScene2dError::Decode(_)
        | PortableScene2dError::Shaping => FrameEncoderError::InvalidInput,
    }
}

fn map_wire_error(_: crate::Wire2dError) -> FrameEncoderError {
    FrameEncoderError::InvalidInput
}
