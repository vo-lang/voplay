use std::collections::BTreeMap;

use crate::{
    control::{ControlKind, StableControlRef},
    presentation::{FrameCommandEncoder, FrameEncodeRequest, FrameEncoderError},
    render_graph::{ResourceUsage, TextureFormat},
    render_world::{Aabb3, RenderComponentKind, RenderWorld, RenderWorldConfig, RenderWorldError},
    RenderEntity,
};

const OBJECT_MAGIC: [u8; 4] = *b"VDR1";
const FRAME_MAGIC: [u8; 4] = *b"VFC1";
const TARGET_BUNDLE_V1_MAGIC: [u8; 4] = *b"VTB1";
const TARGET_BUNDLE_V2_MAGIC: [u8; 4] = *b"VTB2";
const SCENE_3D_FRAME_MAGIC: [u8; 4] = *b"V3F1";
pub const PORTABLE_DRAW_COMPONENT: RenderComponentKind = RenderComponentKind(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetDrawCommands {
    pub target: StableControlRef,
    pub width: u32,
    pub height: u32,
    pub external_surface: bool,
    pub color_format: TextureFormat,
    pub depth_format: Option<TextureFormat>,
    pub sample_count: u8,
    pub usage: ResourceUsage,
    pub commands: Vec<DrawCommand>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrawCommand {
    Clear {
        color: [u8; 4],
    },
    SolidRect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [u8; 4],
    },
    SolidTriangle {
        points: [[i32; 2]; 3],
        color: [u8; 4],
    },
    TexturedRect {
        texture: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32,
        tint: [u8; 4],
    },
    SampledTargetRect {
        target: voplay_protocol::Handle,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32,
        tint: [u8; 4],
    },
}

pub fn portable_frame_commands(bytes: &[u8]) -> Result<&[u8], FrameEncoderError> {
    if bytes.get(..4) != Some(&SCENE_3D_FRAME_MAGIC) {
        return Ok(bytes);
    }
    if bytes.len() < 16 || !matches!(bytes[4], 1..=5) || bytes.get(5..8) != Some(&[1, 0, 0]) {
        return Err(FrameEncoderError::InvalidInput);
    }
    let header_bytes = if bytes[4] >= 2 { 20 } else { 16 };
    if bytes.len() < header_bytes {
        return Err(FrameEncoderError::InvalidInput);
    }
    let fallback_len = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| FrameEncoderError::InvalidInput)?,
    ) as usize;
    let end = header_bytes
        .checked_add(fallback_len)
        .filter(|end| *end <= bytes.len())
        .ok_or(FrameEncoderError::InvalidInput)?;
    if fallback_len == 0 {
        return Err(FrameEncoderError::InvalidInput);
    }
    Ok(&bytes[header_bytes..end])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableFrameEncoderConfig {
    pub max_commands: usize,
    pub max_encoded_bytes: usize,
}

impl Default for PortableFrameEncoderConfig {
    fn default() -> Self {
        Self {
            max_commands: 1_000_000,
            max_encoded_bytes: 8 * 1024 * 1024,
        }
    }
}

pub struct PortableFrameEncoder {
    config: PortableFrameEncoderConfig,
    objects: BTreeMap<RenderEntity, Vec<u8>>,
    closed: bool,
}

impl PortableFrameEncoder {
    pub fn new(config: PortableFrameEncoderConfig) -> Result<Self, FrameEncoderError> {
        if config.max_commands == 0 || config.max_encoded_bytes < 8 {
            return Err(FrameEncoderError::InvalidInput);
        }
        Ok(Self {
            config,
            objects: BTreeMap::new(),
            closed: false,
        })
    }
}

impl FrameCommandEncoder for PortableFrameEncoder {
    fn owner_snapshot(&self) -> crate::presentation::FrameEncoderOwnerSnapshot {
        crate::presentation::FrameEncoderOwnerSnapshot {
            closed: self.closed,
            retained_objects: self.objects.len(),
            retained_assets: 0,
            retained_bytes: self.objects.values().map(Vec::len).sum(),
        }
    }

    fn shutdown(&mut self) {
        self.closed = true;
        self.objects.clear();
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        if request.graph_signature == 0 || request.pulse_id == 0 {
            return Err(FrameEncoderError::InvalidInput);
        }
        let mut objects = if request.full_snapshot {
            BTreeMap::new()
        } else {
            self.objects.clone()
        };
        for entity in request.despawned {
            objects.remove(entity);
        }
        for object in request.objects {
            objects.insert(object.entity, object.bytes.to_vec());
        }
        let mut commands = Vec::new();
        for bytes in objects.values() {
            commands.extend(decode_object_commands(bytes)?);
            if commands.len() > self.config.max_commands {
                return Err(FrameEncoderError::Capacity);
            }
        }
        if let Some(transient) = request.transient {
            commands.extend(decode_object_commands(&transient.bytes)?);
            if commands.len() > self.config.max_commands {
                return Err(FrameEncoderError::Capacity);
            }
        }
        let encoded = encode_frame_commands(&commands, self.config.max_encoded_bytes)?;
        self.objects = objects;
        Ok(encoded)
    }
}

pub struct RetainedPortableFrameEncoder {
    config: PortableFrameEncoderConfig,
    world: RenderWorld,
    view_bounds: Aabb3,
    layers: u64,
}

impl RetainedPortableFrameEncoder {
    pub fn new(
        engine: voplay_protocol::EngineId,
        device_generation: u64,
        config: PortableFrameEncoderConfig,
        world_config: RenderWorldConfig,
        view_bounds: Aabb3,
        layers: u64,
    ) -> Result<Self, FrameEncoderError> {
        if config.max_commands == 0
            || config.max_encoded_bytes < 8
            || !view_bounds.is_valid()
            || layers == 0
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        Ok(Self {
            config,
            world: RenderWorld::new(engine, device_generation, world_config)
                .map_err(map_render_world_error)?,
            view_bounds,
            layers,
        })
    }

    pub const fn world(&self) -> &RenderWorld {
        &self.world
    }

    pub fn set_view(&mut self, bounds: Aabb3, layers: u64) -> Result<(), FrameEncoderError> {
        if !bounds.is_valid() || layers == 0 {
            return Err(FrameEncoderError::InvalidInput);
        }
        self.view_bounds = bounds;
        self.layers = layers;
        Ok(())
    }
}

impl FrameCommandEncoder for RetainedPortableFrameEncoder {
    fn owner_snapshot(&self) -> crate::presentation::FrameEncoderOwnerSnapshot {
        let world = self.world.owner_snapshot();
        crate::presentation::FrameEncoderOwnerSnapshot {
            closed: world.closed,
            retained_objects: world.objects,
            retained_assets: 0,
            retained_bytes: world.component_bytes,
        }
    }

    fn shutdown(&mut self) {
        self.world.shutdown();
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError> {
        if request.engine != self.world.engine()
            || request.graph_signature == 0
            || request.pulse_id == 0
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        let delta = crate::presentation::RenderFrameDelta {
            revision: request.render_revision,
            full_snapshot: request.full_snapshot,
            objects: request
                .objects
                .iter()
                .map(|object| (object.entity, object.bytes.to_vec()))
                .collect(),
            despawned: request.despawned.to_vec(),
        };
        let mut world = self.world.clone();
        world.apply_delta(&delta).map_err(map_render_world_error)?;
        let visible = world
            .visibility(
                request.domain.handle,
                request.control_revision.max(1),
                self.layers,
                self.view_bounds,
            )
            .map_err(map_render_world_error)?
            .visible
            .clone();
        let mut commands = Vec::new();
        for entity in visible {
            let component = world
                .object(entity)
                .and_then(|object| object.components.get(&PORTABLE_DRAW_COMPONENT));
            if let Some(component) = component {
                commands.extend(decode_object_commands(&component.bytes)?);
                if commands.len() > self.config.max_commands {
                    return Err(FrameEncoderError::Capacity);
                }
            }
        }
        if let Some(transient) = request.transient {
            commands.extend(decode_object_commands(&transient.bytes)?);
        }
        if commands.len() > self.config.max_commands {
            return Err(FrameEncoderError::Capacity);
        }
        let encoded = encode_frame_commands(&commands, self.config.max_encoded_bytes)?;
        self.world = world;
        Ok(encoded)
    }
}

fn map_render_world_error(error: RenderWorldError) -> FrameEncoderError {
    match error {
        RenderWorldError::ObjectCapacity
        | RenderWorldError::ComponentCapacity
        | RenderWorldError::ComponentByteCapacity
        | RenderWorldError::TotalByteCapacity
        | RenderWorldError::DirtyCapacity
        | RenderWorldError::ViewCapacity
        | RenderWorldError::SpatialCapacity => FrameEncoderError::Capacity,
        _ => FrameEncoderError::InvalidInput,
    }
}

pub fn encode_object_commands(
    commands: &[DrawCommand],
    max_bytes: usize,
) -> Result<Vec<u8>, FrameEncoderError> {
    encode_commands(OBJECT_MAGIC, commands, max_bytes)
}

pub fn decode_object_commands(bytes: &[u8]) -> Result<Vec<DrawCommand>, FrameEncoderError> {
    decode_commands(bytes, OBJECT_MAGIC)
}

pub fn decode_frame_commands(bytes: &[u8]) -> Result<Vec<DrawCommand>, FrameEncoderError> {
    decode_commands(bytes, FRAME_MAGIC)
}

pub fn encode_frame_commands(
    commands: &[DrawCommand],
    max_bytes: usize,
) -> Result<Vec<u8>, FrameEncoderError> {
    encode_commands(FRAME_MAGIC, commands, max_bytes)
}

pub fn encode_target_frame_bundle(
    targets: &[TargetDrawCommands],
    max_targets: usize,
    max_commands_per_target: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, FrameEncoderError> {
    if targets.is_empty()
        || targets.len() > max_targets
        || max_targets == 0
        || max_commands_per_target == 0
        || max_bytes < 8
    {
        return Err(FrameEncoderError::InvalidInput);
    }
    let engine = targets[0].target.engine;
    let mut seen = std::collections::BTreeSet::new();
    let ordered_targets = order_sampled_targets(targets)?;
    let mut encoded_targets = Vec::with_capacity(targets.len());
    let mut total = 8_usize;
    for target in &ordered_targets {
        if target.target.engine != engine
            || target.target.kind != ControlKind::RenderTarget
            || !target.target.handle.is_valid()
            || target.width == 0
            || target.height == 0
            || !matches!(
                target.color_format,
                TextureFormat::Rgba8 | TextureFormat::Rgba16Float
            )
            || target.depth_format.is_some_and(|format| {
                !matches!(format, TextureFormat::Depth24 | TextureFormat::Depth32Float)
            })
            || !matches!(target.sample_count, 1 | 2 | 4 | 8)
            || target.usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 == 0
            || (target.usage.0 & ResourceUsage::DEPTH_ATTACHMENT.0 != 0)
                != target.depth_format.is_some()
            || !seen.insert(target.target)
            || target.commands.len() > max_commands_per_target
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        let commands = encode_frame_commands(&target.commands, max_bytes)?;
        total = total
            .checked_add(28)
            .and_then(|bytes| bytes.checked_add(commands.len()))
            .filter(|bytes| *bytes <= max_bytes)
            .ok_or(FrameEncoderError::Capacity)?;
        encoded_targets.push(commands);
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&TARGET_BUNDLE_V2_MAGIC);
    bytes.extend_from_slice(&(targets.len() as u32).to_le_bytes());
    for (target, commands) in ordered_targets.into_iter().zip(encoded_targets) {
        bytes.extend_from_slice(&target.target.handle.index.to_le_bytes());
        bytes.extend_from_slice(&target.target.handle.generation.to_le_bytes());
        bytes.extend_from_slice(&target.width.to_le_bytes());
        bytes.extend_from_slice(&target.height.to_le_bytes());
        bytes.push(u8::from(target.external_surface));
        bytes.push(texture_format_tag(target.color_format));
        bytes.push(target.depth_format.map_or(0, texture_format_tag));
        bytes.push(target.sample_count);
        bytes.extend_from_slice(&target.usage.0.to_le_bytes());
        bytes.extend_from_slice(&(commands.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&commands);
    }
    Ok(bytes)
}

pub fn decode_target_frame_bundle(
    bytes: &[u8],
    engine: voplay_protocol::EngineId,
    max_targets: usize,
    max_commands_per_target: usize,
    max_bytes: usize,
) -> Result<Vec<TargetDrawCommands>, FrameEncoderError> {
    if !engine.is_valid()
        || bytes.len() < 8
        || bytes.len() > max_bytes
        || !is_target_frame_bundle(bytes)
        || max_targets == 0
        || max_commands_per_target == 0
    {
        return Err(FrameEncoderError::InvalidInput);
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let version = if bytes.get(..4) == Some(&TARGET_BUNDLE_V2_MAGIC) {
        2
    } else {
        1
    };
    if count == 0 || count > max_targets {
        return Err(FrameEncoderError::Capacity);
    }
    let mut offset = 8_usize;
    let mut targets = Vec::with_capacity(count);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..count {
        let handle = voplay_protocol::Handle {
            index: take_u32(bytes, &mut offset)?,
            generation: take_u32(bytes, &mut offset)?,
        };
        let width = take_u32(bytes, &mut offset)?;
        let height = take_u32(bytes, &mut offset)?;
        let external_surface = match *bytes.get(offset).ok_or(FrameEncoderError::InvalidInput)? {
            0 => false,
            1 => true,
            _ => return Err(FrameEncoderError::InvalidInput),
        };
        offset += 1;
        let (color_format, depth_format, sample_count, usage) = if version == 2 {
            let color_format =
                decode_texture_format(*bytes.get(offset).ok_or(FrameEncoderError::InvalidInput)?)?;
            let depth_format = match *bytes
                .get(offset + 1)
                .ok_or(FrameEncoderError::InvalidInput)?
            {
                0 => None,
                value => Some(decode_texture_format(value)?),
            };
            let sample_count = *bytes
                .get(offset + 2)
                .ok_or(FrameEncoderError::InvalidInput)?;
            offset += 3;
            let usage = ResourceUsage(take_u32(bytes, &mut offset)?);
            (color_format, depth_format, sample_count, usage)
        } else {
            (
                TextureFormat::Rgba8,
                None,
                1,
                ResourceUsage(
                    ResourceUsage::COLOR_ATTACHMENT.0
                        | ResourceUsage::COPY_SRC.0
                        | ResourceUsage::READBACK.0,
                ),
            )
        };
        let command_bytes = take_u32(bytes, &mut offset)? as usize;
        let end = offset
            .checked_add(command_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or(FrameEncoderError::InvalidInput)?;
        let target = StableControlRef {
            engine,
            kind: ControlKind::RenderTarget,
            handle,
        };
        let commands = decode_frame_commands(&bytes[offset..end])?;
        if !handle.is_valid()
            || width == 0
            || height == 0
            || commands.len() > max_commands_per_target
            || !seen.insert(target)
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        targets.push(TargetDrawCommands {
            target,
            width,
            height,
            external_surface,
            color_format,
            depth_format,
            sample_count,
            usage,
            commands,
        });
        offset = end;
    }
    if offset != bytes.len() {
        return Err(FrameEncoderError::InvalidInput);
    }
    let ordered = order_sampled_targets(&targets)?;
    if ordered
        .iter()
        .zip(&targets)
        .any(|(ordered, encoded)| ordered.target != encoded.target)
    {
        return Err(FrameEncoderError::InvalidInput);
    }
    Ok(targets)
}

pub fn is_target_frame_bundle(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(&TARGET_BUNDLE_V1_MAGIC)
        || bytes.get(..4) == Some(&TARGET_BUNDLE_V2_MAGIC)
}

fn order_sampled_targets(
    targets: &[TargetDrawCommands],
) -> Result<Vec<&TargetDrawCommands>, FrameEncoderError> {
    let indices = targets
        .iter()
        .enumerate()
        .map(|(index, target)| (target.target.handle, index))
        .collect::<BTreeMap<_, _>>();
    if indices.len() != targets.len() {
        return Err(FrameEncoderError::InvalidInput);
    }
    let mut indegree = vec![0_usize; targets.len()];
    let mut dependents = vec![Vec::new(); targets.len()];
    for (consumer, target) in targets.iter().enumerate() {
        let mut dependencies = std::collections::BTreeSet::new();
        for command in &target.commands {
            let DrawCommand::SampledTargetRect { target: source, .. } = command else {
                continue;
            };
            let source = *indices.get(source).ok_or(FrameEncoderError::InvalidInput)?;
            if source == consumer
                || targets[source].usage.0 & ResourceUsage::SAMPLED.0 == 0
                || !dependencies.insert(source)
            {
                if source == consumer || targets[source].usage.0 & ResourceUsage::SAMPLED.0 == 0 {
                    return Err(FrameEncoderError::InvalidInput);
                }
                continue;
            }
            indegree[consumer] += 1;
            dependents[source].push(consumer);
        }
    }
    let mut ready = (0..targets.len())
        .filter(|index| indegree[*index] == 0)
        .collect::<std::collections::BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(targets.len());
    while let Some(index) = ready.pop_first() {
        ordered.push(&targets[index]);
        for dependent in &dependents[index] {
            indegree[*dependent] -= 1;
            if indegree[*dependent] == 0 {
                ready.insert(*dependent);
            }
        }
    }
    if ordered.len() != targets.len() {
        return Err(FrameEncoderError::InvalidInput);
    }
    Ok(ordered)
}

fn texture_format_tag(format: TextureFormat) -> u8 {
    match format {
        TextureFormat::Rgba8 => 1,
        TextureFormat::Rgba16Float => 2,
        TextureFormat::Depth24 => 3,
        TextureFormat::Depth32Float => 4,
    }
}

fn decode_texture_format(value: u8) -> Result<TextureFormat, FrameEncoderError> {
    match value {
        1 => Ok(TextureFormat::Rgba8),
        2 => Ok(TextureFormat::Rgba16Float),
        3 => Ok(TextureFormat::Depth24),
        4 => Ok(TextureFormat::Depth32Float),
        _ => Err(FrameEncoderError::InvalidInput),
    }
}

fn encode_commands(
    magic: [u8; 4],
    commands: &[DrawCommand],
    max_bytes: usize,
) -> Result<Vec<u8>, FrameEncoderError> {
    let count = u32::try_from(commands.len()).map_err(|_| FrameEncoderError::Capacity)?;
    let encoded_len = commands.iter().try_fold(8usize, |total, command| {
        total.checked_add(match command {
            DrawCommand::Clear { .. } => 5,
            DrawCommand::SolidRect { .. } => 21,
            DrawCommand::SolidTriangle { .. } => 29,
            DrawCommand::TexturedRect { .. } => 45,
            DrawCommand::SampledTargetRect { .. } => 45,
        })
    });
    let encoded_len = encoded_len
        .filter(|length| *length <= max_bytes)
        .ok_or(FrameEncoderError::Capacity)?;
    let mut output = Vec::with_capacity(encoded_len);
    output.extend_from_slice(&magic);
    output.extend_from_slice(&count.to_le_bytes());
    for command in commands {
        match command {
            DrawCommand::Clear { color } => {
                output.push(1);
                output.extend_from_slice(color);
            }
            DrawCommand::SolidRect {
                x,
                y,
                width,
                height,
                color,
            } => {
                output.push(2);
                output.extend_from_slice(&x.to_le_bytes());
                output.extend_from_slice(&y.to_le_bytes());
                output.extend_from_slice(&width.to_le_bytes());
                output.extend_from_slice(&height.to_le_bytes());
                output.extend_from_slice(color);
            }
            DrawCommand::TexturedRect {
                texture,
                x,
                y,
                width,
                height,
                source_x,
                source_y,
                source_width,
                source_height,
                tint,
            } => {
                output.push(3);
                output.extend_from_slice(&texture.to_le_bytes());
                output.extend_from_slice(&x.to_le_bytes());
                output.extend_from_slice(&y.to_le_bytes());
                output.extend_from_slice(&width.to_le_bytes());
                output.extend_from_slice(&height.to_le_bytes());
                output.extend_from_slice(&source_x.to_le_bytes());
                output.extend_from_slice(&source_y.to_le_bytes());
                output.extend_from_slice(&source_width.to_le_bytes());
                output.extend_from_slice(&source_height.to_le_bytes());
                output.extend_from_slice(tint);
            }
            DrawCommand::SolidTriangle { points, color } => {
                output.push(4);
                for point in points {
                    output.extend_from_slice(&point[0].to_le_bytes());
                    output.extend_from_slice(&point[1].to_le_bytes());
                }
                output.extend_from_slice(color);
            }
            DrawCommand::SampledTargetRect {
                target,
                x,
                y,
                width,
                height,
                source_x,
                source_y,
                source_width,
                source_height,
                tint,
            } => {
                output.push(5);
                output.extend_from_slice(&target.index.to_le_bytes());
                output.extend_from_slice(&target.generation.to_le_bytes());
                output.extend_from_slice(&x.to_le_bytes());
                output.extend_from_slice(&y.to_le_bytes());
                output.extend_from_slice(&width.to_le_bytes());
                output.extend_from_slice(&height.to_le_bytes());
                output.extend_from_slice(&source_x.to_le_bytes());
                output.extend_from_slice(&source_y.to_le_bytes());
                output.extend_from_slice(&source_width.to_le_bytes());
                output.extend_from_slice(&source_height.to_le_bytes());
                output.extend_from_slice(tint);
            }
        }
    }
    Ok(output)
}

fn decode_commands(
    bytes: &[u8],
    expected_magic: [u8; 4],
) -> Result<Vec<DrawCommand>, FrameEncoderError> {
    if bytes.len() < 8 || bytes[..4] != expected_magic {
        return Err(FrameEncoderError::InvalidInput);
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut offset = 8usize;
    let mut commands = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let tag = *bytes.get(offset).ok_or(FrameEncoderError::InvalidInput)?;
        offset += 1;
        let command = match tag {
            1 => DrawCommand::Clear {
                color: take::<4>(bytes, &mut offset)?,
            },
            2 => DrawCommand::SolidRect {
                x: take_u32(bytes, &mut offset)?,
                y: take_u32(bytes, &mut offset)?,
                width: take_u32(bytes, &mut offset)?,
                height: take_u32(bytes, &mut offset)?,
                color: take::<4>(bytes, &mut offset)?,
            },
            3 => DrawCommand::TexturedRect {
                texture: take_u64(bytes, &mut offset)?,
                x: take_u32(bytes, &mut offset)?,
                y: take_u32(bytes, &mut offset)?,
                width: take_u32(bytes, &mut offset)?,
                height: take_u32(bytes, &mut offset)?,
                source_x: take_u32(bytes, &mut offset)?,
                source_y: take_u32(bytes, &mut offset)?,
                source_width: take_u32(bytes, &mut offset)?,
                source_height: take_u32(bytes, &mut offset)?,
                tint: take::<4>(bytes, &mut offset)?,
            },
            4 => DrawCommand::SolidTriangle {
                points: [
                    [take_i32(bytes, &mut offset)?, take_i32(bytes, &mut offset)?],
                    [take_i32(bytes, &mut offset)?, take_i32(bytes, &mut offset)?],
                    [take_i32(bytes, &mut offset)?, take_i32(bytes, &mut offset)?],
                ],
                color: take::<4>(bytes, &mut offset)?,
            },
            5 => DrawCommand::SampledTargetRect {
                target: voplay_protocol::Handle {
                    index: take_u32(bytes, &mut offset)?,
                    generation: take_u32(bytes, &mut offset)?,
                },
                x: take_u32(bytes, &mut offset)?,
                y: take_u32(bytes, &mut offset)?,
                width: take_u32(bytes, &mut offset)?,
                height: take_u32(bytes, &mut offset)?,
                source_x: take_u32(bytes, &mut offset)?,
                source_y: take_u32(bytes, &mut offset)?,
                source_width: take_u32(bytes, &mut offset)?,
                source_height: take_u32(bytes, &mut offset)?,
                tint: take::<4>(bytes, &mut offset)?,
            },
            _ => return Err(FrameEncoderError::Unsupported),
        };
        commands.push(command);
    }
    if offset != bytes.len() {
        return Err(FrameEncoderError::InvalidInput);
    }
    Ok(commands)
}

fn take<const N: usize>(bytes: &[u8], offset: &mut usize) -> Result<[u8; N], FrameEncoderError> {
    let end = offset
        .checked_add(N)
        .ok_or(FrameEncoderError::InvalidInput)?;
    let value = bytes
        .get(*offset..end)
        .ok_or(FrameEncoderError::InvalidInput)?
        .try_into()
        .unwrap();
    *offset = end;
    Ok(value)
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, FrameEncoderError> {
    Ok(u32::from_le_bytes(take(bytes, offset)?))
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, FrameEncoderError> {
    Ok(u64::from_le_bytes(take(bytes, offset)?))
}

fn take_i32(bytes: &[u8], offset: &mut usize) -> Result<i32, FrameEncoderError> {
    Ok(i32::from_le_bytes(take(bytes, offset)?))
}
