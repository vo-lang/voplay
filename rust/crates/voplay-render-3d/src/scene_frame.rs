use voplay_protocol::{EngineId, Handle};
use voplay_runtime::{
    control::StableControlRef,
    render_graph::{ResourceUsage, TextureFormat},
};

use crate::{
    decode_mesh_artifact_3d, encode_mesh_artifact_3d, DirectionalShadowCascade3d, DrawPacket3d,
    LightDescriptor, LightKind, LightPacket3d, MeshArtifact3d, MeshArtifact3dConfig,
    PortableFeatureDraw3d, PortableFeaturePhase3d, PortableFeaturePlan3d, Render3dPlan,
    WireAntiAlias3d, WireEnvironment3d, WireToneMap3d,
};
use voplay_runtime::render_feature::{AttachmentPoint, FeatureId, RealizedFeature};

const MAGIC: &[u8; 4] = b"V3F1";
const HEADER_V1_BYTES: usize = 16;
const HEADER_V2_BYTES: usize = 20;
const VIEW_PREFIX_V2_BYTES: usize = 156;
const VIEW_PREFIX_V3_BYTES: usize = 172;
const VIEW_PREFIX_V5_BYTES: usize = 180;
const DRAW_BYTES: usize = 144;
const LIGHT_BYTES: usize = 76;
const FEATURE_DRAW_BYTES: usize = 136;
const ENVIRONMENT_BYTES: usize = 40;
const SHADOW_CASCADE_BYTES: usize = 92;
const RENDER_FEATURE_PREFIX_BYTES: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct SceneViewFrame3d {
    pub target: StableControlRef,
    pub width: u32,
    pub height: u32,
    pub viewport: [u32; 4],
    pub external_surface: bool,
    pub color_format: TextureFormat,
    pub depth_format: Option<TextureFormat>,
    pub sample_count: u8,
    pub usage: ResourceUsage,
    pub clear_color: [f32; 4],
    pub camera_position_milli: [i64; 3],
    pub view_projection: [[f32; 4]; 4],
    pub plan: Render3dPlan,
    pub features: PortableFeaturePlan3d,
    pub render_features: Vec<RealizedFeature>,
    pub shadow_cascades: Vec<DirectionalShadowCascade3d>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneFrame3d {
    pub fallback_commands: Vec<u8>,
    pub native_overlay_commands: Vec<u8>,
    pub dynamic_meshes: Vec<MeshArtifact3d>,
    pub views: Vec<SceneViewFrame3d>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneFrame3dError {
    Invalid,
    Capacity,
    Truncated,
    TrailingBytes,
}

pub fn is_scene_frame_3d(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(MAGIC)
}

pub fn encode_scene_frame_3d(
    frame: &SceneFrame3d,
    max_views: usize,
    max_draws: usize,
    max_lights: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, SceneFrame3dError> {
    if frame.views.is_empty()
        || frame.views.len() > max_views
        || frame.fallback_commands.is_empty()
        || frame.native_overlay_commands.is_empty()
        || frame.dynamic_meshes.len() > max_draws
        || frame
            .dynamic_meshes
            .iter()
            .enumerate()
            .any(|(index, mesh)| {
                mesh.descriptor.id == 0
                    || frame.dynamic_meshes[..index]
                        .iter()
                        .any(|previous| previous.descriptor.id == mesh.descriptor.id)
            })
        || frame
            .views
            .iter()
            .any(|view| !valid_view(view, max_draws, max_lights))
        || !valid_target_descriptors(&frame.views)
    {
        return Err(SceneFrame3dError::Invalid);
    }
    let mut bytes = Vec::with_capacity(HEADER_V2_BYTES + frame.fallback_commands.len());
    bytes.extend_from_slice(MAGIC);
    bytes.push(5);
    bytes.extend_from_slice(&[1, 0, 0]);
    put_u32(&mut bytes, frame.fallback_commands.len())?;
    put_u32(&mut bytes, frame.views.len())?;
    put_u32(&mut bytes, frame.dynamic_meshes.len())?;
    bytes.extend_from_slice(&frame.fallback_commands);
    for mesh in &frame.dynamic_meshes {
        let encoded = encode_mesh_artifact_3d(mesh, MeshArtifact3dConfig::default())
            .map_err(|_| SceneFrame3dError::Invalid)?;
        put_u32(&mut bytes, encoded.len())?;
        bytes.extend_from_slice(&encoded);
        if bytes.len() > max_bytes {
            return Err(SceneFrame3dError::Capacity);
        }
    }
    for view in &frame.views {
        put_handle(&mut bytes, view.target.handle);
        bytes.extend_from_slice(&view.width.to_le_bytes());
        bytes.extend_from_slice(&view.height.to_le_bytes());
        for value in view.viewport {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(u8::from(view.external_surface));
        bytes.extend_from_slice(&[0; 3]);
        bytes.push(texture_format_tag(view.color_format));
        bytes.push(view.depth_format.map_or(0, texture_format_tag));
        bytes.push(view.sample_count);
        bytes.push(0);
        bytes.extend_from_slice(&view.usage.0.to_le_bytes());
        for value in view.clear_color {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in view.camera_position_milli {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for column in view.view_projection {
            for value in column {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&view.plan.world_revision.to_le_bytes());
        bytes.extend_from_slice(&view.plan.view_revision.to_le_bytes());
        put_u32(&mut bytes, view.plan.opaque.len())?;
        put_u32(&mut bytes, view.plan.transparent.len())?;
        put_u32(&mut bytes, view.plan.shadow_casters.len())?;
        put_u32(&mut bytes, view.plan.lights.len())?;
        for draw in view
            .plan
            .opaque
            .iter()
            .chain(&view.plan.transparent)
            .chain(&view.plan.shadow_casters)
        {
            encode_draw(&mut bytes, *draw);
        }
        for light in &view.plan.lights {
            encode_light(&mut bytes, *light);
        }
        encode_features(&mut bytes, &view.features)?;
        put_u32(&mut bytes, view.render_features.len())?;
        for feature in &view.render_features {
            bytes.extend_from_slice(&feature.feature.0.to_le_bytes());
            bytes.extend_from_slice(&feature.revision.to_le_bytes());
            bytes.extend_from_slice(&feature.pipeline_key.to_le_bytes());
            bytes.push(attachment_tag(feature.attachment));
            bytes.extend_from_slice(&[0; 3]);
            put_u32(&mut bytes, feature.encoded_node.len())?;
            bytes.extend_from_slice(&feature.encoded_node);
        }
        put_u32(&mut bytes, view.shadow_cascades.len())?;
        for cascade in &view.shadow_cascades {
            bytes.extend_from_slice(&cascade.descriptor.index.to_le_bytes());
            bytes.extend_from_slice(&cascade.descriptor.near_milli.to_le_bytes());
            bytes.extend_from_slice(&cascade.descriptor.far_milli.to_le_bytes());
            bytes.extend_from_slice(&cascade.descriptor.texel_world_milli.to_le_bytes());
            for column in cascade.light_view_projection {
                for value in column {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        if bytes.len() > max_bytes {
            return Err(SceneFrame3dError::Capacity);
        }
    }
    put_u32(&mut bytes, frame.native_overlay_commands.len())?;
    bytes.extend_from_slice(&frame.native_overlay_commands);
    if bytes.len() > max_bytes {
        return Err(SceneFrame3dError::Capacity);
    }
    Ok(bytes)
}

pub fn decode_scene_frame_3d(
    bytes: &[u8],
    engine: EngineId,
    max_views: usize,
    max_draws: usize,
    max_lights: usize,
    max_bytes: usize,
) -> Result<SceneFrame3d, SceneFrame3dError> {
    if bytes.len() < HEADER_V1_BYTES
        || bytes.len() > max_bytes
        || bytes.get(..4) != Some(MAGIC)
        || !matches!(bytes[4], 1 | 2 | 3 | 4 | 5)
        || bytes.get(5..8) != Some(&[1, 0, 0])
        || !engine.is_valid()
    {
        return Err(SceneFrame3dError::Invalid);
    }
    let fallback_len = read_u32(bytes, 8)? as usize;
    let view_count = read_u32(bytes, 12)? as usize;
    let dynamic_count = if bytes[4] >= 2 {
        if bytes.len() < HEADER_V2_BYTES {
            return Err(SceneFrame3dError::Truncated);
        }
        read_u32(bytes, 16)? as usize
    } else {
        0
    };
    if fallback_len == 0 || view_count == 0 || view_count > max_views || dynamic_count > max_draws {
        return Err(SceneFrame3dError::Invalid);
    }
    let header_bytes = if bytes[4] >= 2 {
        HEADER_V2_BYTES
    } else {
        HEADER_V1_BYTES
    };
    let mut offset = header_bytes
        .checked_add(fallback_len)
        .filter(|offset| *offset <= bytes.len())
        .ok_or(SceneFrame3dError::Truncated)?;
    let fallback_commands = bytes[header_bytes..offset].to_vec();
    let mut dynamic_meshes = Vec::with_capacity(dynamic_count);
    for _ in 0..dynamic_count {
        let encoded_len = read_u32(bytes, offset)? as usize;
        offset = offset.checked_add(4).ok_or(SceneFrame3dError::Capacity)?;
        let end = offset
            .checked_add(encoded_len)
            .filter(|end| *end <= bytes.len())
            .ok_or(SceneFrame3dError::Truncated)?;
        let mesh = decode_mesh_artifact_3d(&bytes[offset..end], MeshArtifact3dConfig::default())
            .map_err(|_| SceneFrame3dError::Invalid)?;
        if dynamic_meshes
            .iter()
            .any(|previous: &MeshArtifact3d| previous.descriptor.id == mesh.descriptor.id)
        {
            return Err(SceneFrame3dError::Invalid);
        }
        dynamic_meshes.push(mesh);
        offset = end;
    }
    let mut views = Vec::with_capacity(view_count);
    for _ in 0..view_count {
        let prefix_bytes = if bytes[4] >= 5 {
            VIEW_PREFIX_V5_BYTES
        } else if bytes[4] >= 3 {
            VIEW_PREFIX_V3_BYTES
        } else {
            VIEW_PREFIX_V2_BYTES
        };
        let end = offset
            .checked_add(prefix_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or(SceneFrame3dError::Truncated)?;
        let target = StableControlRef {
            engine,
            kind: voplay_runtime::control::ControlKind::RenderTarget,
            handle: read_handle(bytes, offset)?,
        };
        let width = read_u32(bytes, offset + 8)?;
        let height = read_u32(bytes, offset + 12)?;
        let (viewport, suffix) = if bytes[4] >= 3 {
            (
                [
                    read_u32(bytes, offset + 16)?,
                    read_u32(bytes, offset + 20)?,
                    read_u32(bytes, offset + 24)?,
                    read_u32(bytes, offset + 28)?,
                ],
                16,
            )
        } else {
            ([0, 0, width, height], 0)
        };
        let external_surface = match bytes[offset + 16 + suffix] {
            0 => false,
            1 => true,
            _ => return Err(SceneFrame3dError::Invalid),
        };
        if bytes.get(offset + 17 + suffix..offset + 20 + suffix) != Some(&[0; 3]) {
            return Err(SceneFrame3dError::Invalid);
        }
        let descriptor_suffix = if bytes[4] >= 5 { 8 } else { 0 };
        let (color_format, depth_format, sample_count, usage) = if bytes[4] >= 5 {
            if bytes[offset + 23 + suffix] != 0 {
                return Err(SceneFrame3dError::Invalid);
            }
            (
                decode_texture_format(bytes[offset + 20 + suffix])?,
                match bytes[offset + 21 + suffix] {
                    0 => None,
                    value => Some(decode_texture_format(value)?),
                },
                bytes[offset + 22 + suffix],
                ResourceUsage(read_u32(bytes, offset + 24 + suffix)?),
            )
        } else {
            (
                TextureFormat::Rgba8,
                Some(TextureFormat::Depth32Float),
                1,
                ResourceUsage(
                    ResourceUsage::COLOR_ATTACHMENT.0
                        | ResourceUsage::DEPTH_ATTACHMENT.0
                        | ResourceUsage::COPY_SRC.0,
                ),
            )
        };
        let clear_color = read_f32_array::<4>(bytes, offset + 20 + suffix + descriptor_suffix)?;
        let camera_position_milli =
            read_i64_array::<3>(bytes, offset + 36 + suffix + descriptor_suffix)?;
        let flat_view_projection =
            read_f32_array::<16>(bytes, offset + 60 + suffix + descriptor_suffix)?;
        let mut view_projection = [[0.0; 4]; 4];
        for (index, value) in flat_view_projection.into_iter().enumerate() {
            view_projection[index / 4][index % 4] = value;
        }
        let world_revision = read_u64(bytes, offset + 124 + suffix + descriptor_suffix)?;
        let view_revision = read_u64(bytes, offset + 132 + suffix + descriptor_suffix)?;
        let counts = [
            read_u32(bytes, offset + 140 + suffix + descriptor_suffix)? as usize,
            read_u32(bytes, offset + 144 + suffix + descriptor_suffix)? as usize,
            read_u32(bytes, offset + 148 + suffix + descriptor_suffix)? as usize,
            read_u32(bytes, offset + 152 + suffix + descriptor_suffix)? as usize,
        ];
        offset = end;
        let draw_count = counts[0]
            .checked_add(counts[1])
            .and_then(|count| count.checked_add(counts[2]))
            .filter(|count| *count <= max_draws.saturating_mul(2))
            .ok_or(SceneFrame3dError::Capacity)?;
        if counts[0].saturating_add(counts[1]) > max_draws || counts[3] > max_lights {
            return Err(SceneFrame3dError::Capacity);
        }
        let mut draws = Vec::with_capacity(draw_count);
        for _ in 0..draw_count {
            draws.push(decode_draw(bytes, engine, &mut offset)?);
        }
        let mut lights = Vec::with_capacity(counts[3]);
        for _ in 0..counts[3] {
            lights.push(decode_light(bytes, engine, &mut offset)?);
        }
        let features = decode_features(bytes, &mut offset, max_draws)?;
        let render_features = if bytes[4] >= 4 {
            let count = read_u32(bytes, offset)? as usize;
            offset = offset.checked_add(4).ok_or(SceneFrame3dError::Capacity)?;
            if count > max_draws {
                return Err(SceneFrame3dError::Capacity);
            }
            let mut realized = Vec::with_capacity(count);
            for _ in 0..count {
                let start = offset;
                let prefix_end = start
                    .checked_add(RENDER_FEATURE_PREFIX_BYTES)
                    .filter(|end| *end <= bytes.len())
                    .ok_or(SceneFrame3dError::Truncated)?;
                let encoded_len = read_u32(bytes, start + 28)? as usize;
                let end = prefix_end
                    .checked_add(encoded_len)
                    .filter(|end| *end <= bytes.len())
                    .ok_or(SceneFrame3dError::Truncated)?;
                let attachment = decode_attachment(bytes[start + 24])?;
                if bytes.get(start + 25..start + 28) != Some(&[0; 3]) || encoded_len == 0 {
                    return Err(SceneFrame3dError::Invalid);
                }
                realized.push(RealizedFeature {
                    feature: FeatureId(read_u64(bytes, start)?),
                    revision: read_u64(bytes, start + 8)?,
                    pipeline_key: read_u64(bytes, start + 16)?,
                    attachment,
                    encoded_node: bytes[prefix_end..end].to_vec(),
                });
                offset = end;
            }
            realized
        } else {
            Vec::new()
        };
        let shadow_count = read_u32(bytes, offset)? as usize;
        offset = offset.checked_add(4).ok_or(SceneFrame3dError::Capacity)?;
        if shadow_count > 4 {
            return Err(SceneFrame3dError::Capacity);
        }
        let mut shadow_cascades = Vec::with_capacity(shadow_count);
        for _ in 0..shadow_count {
            let start = offset;
            offset = start
                .checked_add(SHADOW_CASCADE_BYTES)
                .filter(|end| *end <= bytes.len())
                .ok_or(SceneFrame3dError::Truncated)?;
            let flat = read_f32_array::<16>(bytes, start + 28)?;
            let mut light_view_projection = [[0.0; 4]; 4];
            for (index, value) in flat.into_iter().enumerate() {
                light_view_projection[index / 4][index % 4] = value;
            }
            shadow_cascades.push(DirectionalShadowCascade3d {
                descriptor: crate::ShadowCascade {
                    index: read_u32(bytes, start)?,
                    near_milli: read_u64(bytes, start + 4)?,
                    far_milli: read_u64(bytes, start + 12)?,
                    texel_world_milli: read_u64(bytes, start + 20)?,
                },
                light_view_projection,
            });
        }
        let transparent_start = counts[0];
        let shadow_start = transparent_start + counts[1];
        let view = SceneViewFrame3d {
            target,
            width,
            height,
            viewport,
            external_surface,
            color_format,
            depth_format,
            sample_count,
            usage,
            clear_color,
            camera_position_milli,
            view_projection,
            plan: Render3dPlan {
                world_revision,
                view_revision,
                opaque: draws[..transparent_start].to_vec(),
                transparent: draws[transparent_start..shadow_start].to_vec(),
                shadow_casters: draws[shadow_start..].to_vec(),
                lights,
            },
            features,
            render_features,
            shadow_cascades,
        };
        if !valid_view(&view, max_draws, max_lights) {
            return Err(SceneFrame3dError::Invalid);
        }
        views.push(view);
    }
    let overlay_len = read_u32(bytes, offset)? as usize;
    offset = offset.checked_add(4).ok_or(SceneFrame3dError::Capacity)?;
    let end = offset
        .checked_add(overlay_len)
        .filter(|end| *end == bytes.len())
        .ok_or(SceneFrame3dError::TrailingBytes)?;
    if overlay_len == 0 {
        return Err(SceneFrame3dError::Invalid);
    }
    let frame = SceneFrame3d {
        fallback_commands,
        native_overlay_commands: bytes[offset..end].to_vec(),
        dynamic_meshes,
        views,
    };
    if !valid_target_descriptors(&frame.views) {
        return Err(SceneFrame3dError::Invalid);
    }
    Ok(frame)
}

fn valid_target_descriptors(views: &[SceneViewFrame3d]) -> bool {
    views.iter().enumerate().all(|(index, view)| {
        views[..index]
            .iter()
            .find(|previous| previous.target == view.target)
            .is_none_or(|previous| {
                previous.width == view.width
                    && previous.height == view.height
                    && previous.external_surface == view.external_surface
                    && previous.color_format == view.color_format
                    && previous.depth_format == view.depth_format
                    && previous.sample_count == view.sample_count
                    && previous.usage == view.usage
            })
    })
}

fn valid_view(view: &SceneViewFrame3d, max_draws: usize, max_lights: usize) -> bool {
    view.target.handle.is_valid()
        && view.width > 0
        && view.height > 0
        && view.viewport[2] > 0
        && view.viewport[3] > 0
        && view.viewport[0]
            .checked_add(view.viewport[2])
            .is_some_and(|right| right <= view.width)
        && view.viewport[1]
            .checked_add(view.viewport[3])
            .is_some_and(|bottom| bottom <= view.height)
        && matches!(
            view.color_format,
            TextureFormat::Rgba8 | TextureFormat::Rgba16Float
        )
        && view.depth_format.is_none_or(|format| {
            matches!(format, TextureFormat::Depth24 | TextureFormat::Depth32Float)
        })
        && matches!(view.sample_count, 1 | 2 | 4 | 8)
        && view.usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 != 0
        && (view.usage.0 & ResourceUsage::DEPTH_ATTACHMENT.0 != 0) == view.depth_format.is_some()
        && view.clear_color.iter().all(|value| value.is_finite())
        && view
            .view_projection
            .iter()
            .flatten()
            .all(|value| value.is_finite())
        && view.plan.world_revision > 0
        && view.plan.view_revision > 0
        && view
            .plan
            .opaque
            .len()
            .saturating_add(view.plan.transparent.len())
            <= max_draws
        && view.plan.shadow_casters.len() <= max_draws
        && view.plan.lights.len() <= max_lights
        && view.features.draws.len() <= max_draws
        && view.render_features.len() <= max_draws
        && view.render_features.iter().all(|feature| {
            feature.feature.is_valid()
                && feature.revision > 0
                && feature.pipeline_key > 0
                && !feature.encoded_node.is_empty()
        })
        && view.shadow_cascades.len() <= 4
        && view
            .shadow_cascades
            .iter()
            .enumerate()
            .all(|(index, cascade)| {
                cascade.descriptor.index == index as u32
                    && cascade.descriptor.near_milli < cascade.descriptor.far_milli
                    && cascade.descriptor.texel_world_milli != 0
                    && (index == 0
                        || view.shadow_cascades[index - 1].descriptor.far_milli
                            <= cascade.descriptor.near_milli)
                    && cascade
                        .light_view_projection
                        .iter()
                        .flatten()
                        .all(|value| value.is_finite())
            })
}

fn attachment_tag(value: AttachmentPoint) -> u8 {
    match value {
        AttachmentPoint::BeforeDepth => 1,
        AttachmentPoint::AfterOpaque => 2,
        AttachmentPoint::BeforeTransparent => 3,
        AttachmentPoint::BeforePost => 4,
        AttachmentPoint::AfterPost => 5,
        AttachmentPoint::Overlay => 6,
    }
}

fn texture_format_tag(value: TextureFormat) -> u8 {
    match value {
        TextureFormat::Rgba8 => 1,
        TextureFormat::Rgba16Float => 2,
        TextureFormat::Depth24 => 3,
        TextureFormat::Depth32Float => 4,
    }
}

fn decode_texture_format(value: u8) -> Result<TextureFormat, SceneFrame3dError> {
    match value {
        1 => Ok(TextureFormat::Rgba8),
        2 => Ok(TextureFormat::Rgba16Float),
        3 => Ok(TextureFormat::Depth24),
        4 => Ok(TextureFormat::Depth32Float),
        _ => Err(SceneFrame3dError::Invalid),
    }
}

fn decode_attachment(value: u8) -> Result<AttachmentPoint, SceneFrame3dError> {
    Ok(match value {
        1 => AttachmentPoint::BeforeDepth,
        2 => AttachmentPoint::AfterOpaque,
        3 => AttachmentPoint::BeforeTransparent,
        4 => AttachmentPoint::BeforePost,
        5 => AttachmentPoint::AfterPost,
        6 => AttachmentPoint::Overlay,
        _ => return Err(SceneFrame3dError::Invalid),
    })
}

fn encode_features(
    bytes: &mut Vec<u8>,
    features: &PortableFeaturePlan3d,
) -> Result<(), SceneFrame3dError> {
    bytes.extend_from_slice(&features.scene_revision.to_le_bytes());
    put_u32(bytes, features.draws.len())?;
    bytes.push(u8::from(features.environment.is_some()));
    bytes.extend_from_slice(&[0; 3]);
    if let Some(environment) = features.environment {
        for color in environment.fog_color {
            bytes.extend_from_slice(&color.to_le_bytes());
        }
        bytes.extend_from_slice(&[0; 2]);
        bytes.extend_from_slice(&environment.fog_start_milli.to_le_bytes());
        bytes.extend_from_slice(&environment.fog_end_milli.to_le_bytes());
        bytes.extend_from_slice(&environment.exposure_milli.to_le_bytes());
        bytes.push(match environment.tone_map {
            WireToneMap3d::Linear => 1,
            WireToneMap3d::Reinhard => 2,
            WireToneMap3d::Aces => 3,
        });
        bytes.push(match environment.anti_alias {
            WireAntiAlias3d::None => 1,
            WireAntiAlias3d::Fxaa => 2,
            WireAntiAlias3d::Taa => 3,
            WireAntiAlias3d::Msaa => 4,
        });
        bytes.extend_from_slice(&[0; 2]);
        bytes.extend_from_slice(&environment.bloom_threshold.to_le_bytes());
        bytes.extend_from_slice(&environment.bloom_strength.to_le_bytes());
        bytes.extend_from_slice(&environment.msaa_samples.to_le_bytes());
    }
    for draw in &features.draws {
        bytes.extend_from_slice(&draw.source.to_le_bytes());
        bytes.extend_from_slice(&draw.serial.to_le_bytes());
        bytes.push(match draw.phase {
            PortableFeaturePhase3d::Terrain => 1,
            PortableFeaturePhase3d::Scatter => 2,
            PortableFeaturePhase3d::DecalBeforeLighting => 3,
            PortableFeaturePhase3d::Particle => 4,
            PortableFeaturePhase3d::DecalAfterLighting => 5,
        });
        bytes.push(u8::from(draw.casts_shadow));
        bytes.extend_from_slice(&[0; 6]);
        bytes.extend_from_slice(&draw.mesh.to_le_bytes());
        bytes.extend_from_slice(&draw.material.to_le_bytes());
        for value in draw.model_milli {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(())
}

fn decode_features(
    bytes: &[u8],
    offset: &mut usize,
    max_draws: usize,
) -> Result<PortableFeaturePlan3d, SceneFrame3dError> {
    let start = *offset;
    let prefix_end = start
        .checked_add(16)
        .filter(|end| *end <= bytes.len())
        .ok_or(SceneFrame3dError::Truncated)?;
    let scene_revision = read_u64(bytes, start)?;
    let count = read_u32(bytes, start + 8)? as usize;
    if count > max_draws
        || bytes[start + 12] > 1
        || bytes.get(start + 13..start + 16) != Some(&[0; 3])
    {
        return Err(SceneFrame3dError::Invalid);
    }
    *offset = prefix_end;
    let environment = if bytes[start + 12] == 1 {
        let environment_start = *offset;
        *offset = environment_start
            .checked_add(ENVIRONMENT_BYTES)
            .filter(|end| *end <= bytes.len())
            .ok_or(SceneFrame3dError::Truncated)?;
        if bytes.get(environment_start + 6..environment_start + 8) != Some(&[0; 2])
            || bytes.get(environment_start + 30..environment_start + 32) != Some(&[0; 2])
        {
            return Err(SceneFrame3dError::Invalid);
        }
        let tone_map = match bytes[environment_start + 28] {
            1 => WireToneMap3d::Linear,
            2 => WireToneMap3d::Reinhard,
            3 => WireToneMap3d::Aces,
            _ => return Err(SceneFrame3dError::Invalid),
        };
        let anti_alias = match bytes[environment_start + 29] {
            1 => WireAntiAlias3d::None,
            2 => WireAntiAlias3d::Fxaa,
            3 => WireAntiAlias3d::Taa,
            4 => WireAntiAlias3d::Msaa,
            _ => return Err(SceneFrame3dError::Invalid),
        };
        Some(WireEnvironment3d {
            fog_color: [
                read_u16(bytes, environment_start)?,
                read_u16(bytes, environment_start + 2)?,
                read_u16(bytes, environment_start + 4)?,
            ],
            fog_start_milli: read_u64(bytes, environment_start + 8)?,
            fog_end_milli: read_u64(bytes, environment_start + 16)?,
            exposure_milli: read_i32(bytes, environment_start + 24)?,
            tone_map,
            bloom_threshold: read_u16(bytes, environment_start + 32)?,
            bloom_strength: read_u16(bytes, environment_start + 34)?,
            anti_alias,
            msaa_samples: read_u32(bytes, environment_start + 36)?,
        })
    } else {
        None
    };
    let mut draws = Vec::with_capacity(count);
    for _ in 0..count {
        let start = *offset;
        *offset = start
            .checked_add(FEATURE_DRAW_BYTES)
            .filter(|end| *end <= bytes.len())
            .ok_or(SceneFrame3dError::Truncated)?;
        let phase = match bytes[start + 16] {
            1 => PortableFeaturePhase3d::Terrain,
            2 => PortableFeaturePhase3d::Scatter,
            3 => PortableFeaturePhase3d::DecalBeforeLighting,
            4 => PortableFeaturePhase3d::Particle,
            5 => PortableFeaturePhase3d::DecalAfterLighting,
            _ => return Err(SceneFrame3dError::Invalid),
        };
        if bytes[start + 17] > 1 || bytes.get(start + 18..start + 24) != Some(&[0; 6]) {
            return Err(SceneFrame3dError::Invalid);
        }
        let draw = PortableFeatureDraw3d {
            source: read_u64(bytes, start)?,
            serial: read_u64(bytes, start + 8)?,
            phase,
            casts_shadow: bytes[start + 17] == 1,
            mesh: read_u64(bytes, start + 24)?,
            material: read_u64(bytes, start + 32)?,
            model_milli: read_i64_array(bytes, start + 40)?,
        };
        if draw.source == 0 || draw.mesh == 0 || draw.material == 0 {
            return Err(SceneFrame3dError::Invalid);
        }
        draws.push(draw);
    }
    Ok(PortableFeaturePlan3d {
        scene_revision,
        draws,
        environment,
    })
}

fn encode_draw(bytes: &mut Vec<u8>, draw: DrawPacket3d) {
    put_handle(bytes, draw.entity.entity);
    bytes.extend_from_slice(&draw.mesh.to_le_bytes());
    bytes.extend_from_slice(&draw.material.to_le_bytes());
    bytes.extend_from_slice(&draw.pipeline_key.to_le_bytes());
    bytes.extend_from_slice(&draw.depth_key.to_le_bytes());
    bytes.push(u8::from(draw.casts_shadow));
    bytes.push(u8::from(draw.receives_shadow));
    bytes.extend_from_slice(&[0; 6]);
    for value in draw.model_milli {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn decode_draw(
    bytes: &[u8],
    engine: EngineId,
    offset: &mut usize,
) -> Result<DrawPacket3d, SceneFrame3dError> {
    let start = *offset;
    *offset = start
        .checked_add(DRAW_BYTES)
        .filter(|end| *end <= bytes.len())
        .ok_or(SceneFrame3dError::Truncated)?;
    let flags = (bytes[start + 40], bytes[start + 41]);
    if flags.0 > 1 || flags.1 > 1 || bytes.get(start + 42..start + 48) != Some(&[0; 6]) {
        return Err(SceneFrame3dError::Invalid);
    }
    Ok(DrawPacket3d {
        entity: voplay_runtime::RenderEntity {
            engine,
            entity: read_handle(bytes, start)?,
        },
        mesh: read_u64(bytes, start + 8)?,
        material: read_u64(bytes, start + 16)?,
        pipeline_key: read_u64(bytes, start + 24)?,
        depth_key: read_i64(bytes, start + 32)?,
        casts_shadow: flags.0 == 1,
        receives_shadow: flags.1 == 1,
        model_milli: read_i64_array(bytes, start + 48)?,
    })
}

fn encode_light(bytes: &mut Vec<u8>, light: LightPacket3d) {
    put_handle(bytes, light.entity.entity);
    let (tag, angle) = match light.descriptor.kind {
        LightKind::Directional => (1, 0),
        LightKind::Point => (2, 0),
        LightKind::Spot { outer_angle_q16 } => (3, outer_angle_q16),
    };
    bytes.push(tag);
    bytes.push(u8::from(light.descriptor.casts_shadow));
    bytes.extend_from_slice(&angle.to_le_bytes());
    for value in light.descriptor.color {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[0; 2]);
    bytes.extend_from_slice(&light.descriptor.intensity_milli.to_le_bytes());
    bytes.extend_from_slice(&light.descriptor.range_milli.to_le_bytes());
    for value in light.position_milli {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in light.direction_milli {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn decode_light(
    bytes: &[u8],
    engine: EngineId,
    offset: &mut usize,
) -> Result<LightPacket3d, SceneFrame3dError> {
    let start = *offset;
    *offset = start
        .checked_add(LIGHT_BYTES)
        .filter(|end| *end <= bytes.len())
        .ok_or(SceneFrame3dError::Truncated)?;
    let angle = read_u16(bytes, start + 10)?;
    let kind = match bytes[start + 8] {
        1 if angle == 0 => LightKind::Directional,
        2 if angle == 0 => LightKind::Point,
        3 if angle != 0 => LightKind::Spot {
            outer_angle_q16: angle,
        },
        _ => return Err(SceneFrame3dError::Invalid),
    };
    if bytes[start + 9] > 1 || bytes.get(start + 18..start + 20) != Some(&[0; 2]) {
        return Err(SceneFrame3dError::Invalid);
    }
    Ok(LightPacket3d {
        entity: voplay_runtime::RenderEntity {
            engine,
            entity: read_handle(bytes, start)?,
        },
        descriptor: LightDescriptor {
            kind,
            color: [
                read_u16(bytes, start + 12)?,
                read_u16(bytes, start + 14)?,
                read_u16(bytes, start + 16)?,
            ],
            intensity_milli: read_u32(bytes, start + 20)?,
            range_milli: read_u32(bytes, start + 24)?,
            casts_shadow: bytes[start + 9] == 1,
        },
        position_milli: read_i64_array(bytes, start + 28)?,
        direction_milli: read_i64_array(bytes, start + 52)?,
    })
}

fn put_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn put_u32(bytes: &mut Vec<u8>, value: usize) -> Result<(), SceneFrame3dError> {
    bytes.extend_from_slice(
        &u32::try_from(value)
            .map_err(|_| SceneFrame3dError::Capacity)?
            .to_le_bytes(),
    );
    Ok(())
}

fn read_handle(bytes: &[u8], offset: usize) -> Result<Handle, SceneFrame3dError> {
    let handle = Handle {
        index: read_u32(bytes, offset)?,
        generation: read_u32(bytes, offset + 4)?,
    };
    if !handle.is_valid() {
        return Err(SceneFrame3dError::Invalid);
    }
    Ok(handle)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, SceneFrame3dError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(SceneFrame3dError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SceneFrame3dError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(SceneFrame3dError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, SceneFrame3dError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(SceneFrame3dError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, SceneFrame3dError> {
    Ok(i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(SceneFrame3dError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, SceneFrame3dError> {
    Ok(i32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(SceneFrame3dError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_f32_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
) -> Result<[f32; N], SceneFrame3dError> {
    let mut result = [0.0; N];
    for (index, value) in result.iter_mut().enumerate() {
        *value = f32::from_le_bytes(
            bytes
                .get(offset + index * 4..offset + index * 4 + 4)
                .ok_or(SceneFrame3dError::Truncated)?
                .try_into()
                .unwrap(),
        );
    }
    Ok(result)
}

fn read_i64_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
) -> Result<[i64; N], SceneFrame3dError> {
    let mut result = [0; N];
    for (index, value) in result.iter_mut().enumerate() {
        *value = read_i64(bytes, offset + index * 8)?;
    }
    Ok(result)
}
