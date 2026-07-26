use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

const GLB_JSON_CHUNK: u32 = 0x4e4f_534a;
const GLB_BIN_CHUNK: u32 = 0x004e_4942;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfImportConfig {
    pub max_source_bytes: usize,
    pub max_json_bytes: usize,
    pub max_binary_bytes: usize,
    pub max_objects: usize,
    pub max_dependencies: usize,
}

impl Default for GltfImportConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 256 * 1024 * 1024,
            max_json_bytes: 32 * 1024 * 1024,
            max_binary_bytes: 256 * 1024 * 1024,
            max_objects: 1_000_000,
            max_dependencies: 65_536,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfContainer {
    Json,
    Glb { declared_bytes: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfDependency {
    pub uri: String,
    pub embedded_data: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfAlphaMode {
    Opaque,
    Mask { cutoff_q16: u16 },
    Blend,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfMaterialPlan {
    pub name: Option<String>,
    pub alpha: GltfAlphaMode,
    pub double_sided: bool,
    pub unlit: bool,
    pub base_color: [u8; 4],
    pub metallic_q16: u16,
    pub roughness_q16: u16,
    pub emissive_q16: [u16; 3],
    pub normal_scale_q16: u16,
    pub occlusion_strength_q16: u16,
    pub textures: [Option<usize>; 5],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfImagePlan {
    pub uri: Option<String>,
    pub buffer_view: Option<usize>,
    pub mime_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfTexturePlan {
    pub image: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfSkinPlan {
    pub name: Option<String>,
    pub joints: Vec<usize>,
    pub skeleton: Option<usize>,
    pub inverse_bind_accessor: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfSkinnedNodePlan {
    pub node: usize,
    pub mesh: usize,
    pub skin: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfInterpolation {
    Linear,
    Step,
    CubicSpline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfAnimationTarget {
    Translation,
    Rotation,
    Scale,
    Weights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfAnimationSamplerPlan {
    pub input_accessor: usize,
    pub output_accessor: usize,
    pub interpolation: GltfInterpolation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfAnimationChannelPlan {
    pub sampler: usize,
    pub node: usize,
    pub target: GltfAnimationTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfAnimationPlan {
    pub name: Option<String>,
    pub samplers: Vec<GltfAnimationSamplerPlan>,
    pub channels: Vec<GltfAnimationChannelPlan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfNodeTrsPlan {
    pub translation_milli: [i64; 3],
    pub rotation_q30: [i64; 4],
    pub scale_milli: [i64; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfNodePlan {
    pub name: Option<String>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    pub children: Vec<usize>,
    pub local_matrix_milli: [i64; 16],
    pub rest_trs: Option<GltfNodeTrsPlan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfScenePlan {
    pub name: Option<String>,
    pub roots: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfPrimitivePlan {
    pub mesh: usize,
    pub primitive: usize,
    pub attributes: Vec<String>,
    pub position_accessor: usize,
    pub normal_accessor: Option<usize>,
    pub texcoord_accessor: Option<usize>,
    pub joints_accessor: Option<usize>,
    pub weights_accessor: Option<usize>,
    pub indices: Option<usize>,
    pub material: Option<usize>,
    pub mode: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfMeshArtifact {
    pub mesh: usize,
    pub primitive: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub texcoords: Vec<[f32; 2]>,
    pub joints: Option<Vec<[u16; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfImageArtifact {
    pub image: usize,
    pub mime_type: Option<String>,
    pub encoded_bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfSkinArtifact {
    pub skin: usize,
    pub joints: Vec<usize>,
    pub skeleton: Option<usize>,
    pub inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GltfAnimationValues {
    Scalar(Vec<f32>),
    Vec3(Vec<[f32; 3]>),
    Vec4(Vec<[f32; 4]>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfAnimationChannelArtifact {
    pub node: usize,
    pub target: GltfAnimationTarget,
    pub interpolation: GltfInterpolation,
    pub times_seconds: Vec<f32>,
    pub values: GltfAnimationValues,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfAnimationArtifact {
    pub animation: usize,
    pub name: Option<String>,
    pub channels: Vec<GltfAnimationChannelArtifact>,
    pub duration_seconds: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GltfSampledAnimationValue {
    Weights(Vec<f32>),
    Translation([f32; 3]),
    Rotation([f32; 4]),
    Scale([f32; 3]),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GltfDocumentSummary {
    pub scenes: usize,
    pub nodes: usize,
    pub meshes: usize,
    pub primitives: usize,
    pub materials: usize,
    pub textures: usize,
    pub images: usize,
    pub skins: usize,
    pub animations: usize,
    pub accessors: usize,
    pub buffer_views: usize,
    pub buffers: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GltfImportPlan {
    pub container: GltfContainer,
    pub summary: GltfDocumentSummary,
    pub dependencies: Vec<GltfDependency>,
    pub materials: Vec<GltfMaterialPlan>,
    pub images: Vec<GltfImagePlan>,
    pub textures: Vec<GltfTexturePlan>,
    pub skins: Vec<GltfSkinPlan>,
    pub skinned_nodes: Vec<GltfSkinnedNodePlan>,
    pub animations: Vec<GltfAnimationPlan>,
    pub nodes: Vec<GltfNodePlan>,
    pub scenes: Vec<GltfScenePlan>,
    pub primitives: Vec<GltfPrimitivePlan>,
    pub default_scene: Option<usize>,
    pub json_bytes: Vec<u8>,
    pub binary_chunk: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfImportError {
    InvalidConfig,
    Empty,
    Capacity,
    InvalidJsonEnvelope,
    InvalidJson,
    InvalidGlbHeader,
    InvalidGlbChunk,
    LengthMismatch,
    UnsupportedVersion,
    InvalidReference,
    InvalidAccessor,
    InvalidPrimitive,
    NodeCycle,
    UnsafeUri,
    MissingBinaryChunk,
    UnsupportedExtension,
}

pub fn validate_container(
    source: &[u8],
    config: GltfImportConfig,
) -> Result<GltfContainer, GltfImportError> {
    validate_config(config)?;
    if source.is_empty() {
        return Err(GltfImportError::Empty);
    }
    if source.len() > config.max_source_bytes {
        return Err(GltfImportError::Capacity);
    }
    if source.starts_with(b"glTF") {
        if source.len() < 12 {
            return Err(GltfImportError::InvalidGlbHeader);
        }
        let version = read_u32(source, 4)?;
        let declared = read_u32(source, 8)?;
        if version != 2 || declared < 12 {
            return Err(GltfImportError::InvalidGlbHeader);
        }
        if usize::try_from(declared).ok() != Some(source.len()) {
            return Err(GltfImportError::LengthMismatch);
        }
        return Ok(GltfContainer::Glb {
            declared_bytes: declared,
        });
    }
    let first = source
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .ok_or(GltfImportError::Empty)?;
    let last = source
        .iter()
        .copied()
        .rfind(|byte| !byte.is_ascii_whitespace())
        .ok_or(GltfImportError::Empty)?;
    if first != b'{' || last != b'}' {
        return Err(GltfImportError::InvalidJsonEnvelope);
    }
    Ok(GltfContainer::Json)
}

pub fn import(source: &[u8], config: GltfImportConfig) -> Result<GltfImportPlan, GltfImportError> {
    let container = validate_container(source, config)?;
    let (json_bytes, binary_chunk) = match container {
        GltfContainer::Json => {
            if source.len() > config.max_json_bytes {
                return Err(GltfImportError::Capacity);
            }
            (source.to_vec(), None)
        }
        GltfContainer::Glb { .. } => decode_glb(source, config)?,
    };
    let root: Value =
        serde_json::from_slice(&json_bytes).map_err(|_| GltfImportError::InvalidJson)?;
    let root = root
        .as_object()
        .ok_or(GltfImportError::InvalidJsonEnvelope)?;
    validate_asset(root)?;
    validate_extensions(root)?;
    let summary = summarize(root, config)?;
    let dependencies = collect_dependencies(root, config)?;
    validate_buffers(root, binary_chunk.as_deref())?;
    validate_buffer_views(root)?;
    validate_accessors(root)?;
    let primitives = validate_meshes(root)?;
    validate_nodes(root)?;
    validate_scenes(root)?;
    validate_skins(root)?;
    validate_animations(root)?;
    validate_texture_references(root)?;
    let materials = import_materials(root)?;
    let images = import_images(root)?;
    let textures = import_textures(root)?;
    let skins = import_skins(root)?;
    let skinned_nodes = import_skinned_nodes(root)?;
    let animations = import_animations(root)?;
    let nodes = import_nodes(root)?;
    let scenes = import_scenes(root)?;
    let default_scene = optional_index(root.get("scene"), summary.scenes)?;
    Ok(GltfImportPlan {
        container,
        summary,
        dependencies,
        materials,
        images,
        textures,
        skins,
        skinned_nodes,
        animations,
        nodes,
        scenes,
        primitives,
        default_scene,
        json_bytes,
        binary_chunk,
    })
}

fn import_images(root: &Map<String, Value>) -> Result<Vec<GltfImagePlan>, GltfImportError> {
    array(root.get("images"))?
        .iter()
        .map(|image| {
            let image = object(Some(image))?;
            let uri = image
                .get("uri")
                .map(|value| string(Some(value)).map(str::to_owned))
                .transpose()?;
            let buffer_view =
                optional_index(image.get("bufferView"), array_len(root.get("bufferViews"))?)?;
            if uri.is_some() == buffer_view.is_some() {
                return Err(GltfImportError::InvalidReference);
            }
            let explicit_mime = image
                .get("mimeType")
                .map(|value| string(Some(value)).map(str::to_owned))
                .transpose()?;
            let data_mime = uri
                .as_deref()
                .and_then(data_uri_mime_type)
                .map(str::to_owned);
            Ok(GltfImagePlan {
                uri,
                buffer_view,
                mime_type: explicit_mime.or(data_mime),
            })
        })
        .collect()
}

fn import_textures(root: &Map<String, Value>) -> Result<Vec<GltfTexturePlan>, GltfImportError> {
    let images = array_len(root.get("images"))?;
    array(root.get("textures"))?
        .iter()
        .map(|texture| {
            let texture = object(Some(texture))?;
            Ok(GltfTexturePlan {
                image: required_index(texture.get("source"), images)?,
            })
        })
        .collect()
}

fn import_skins(root: &Map<String, Value>) -> Result<Vec<GltfSkinPlan>, GltfImportError> {
    let nodes = array_len(root.get("nodes"))?;
    let accessors = array_len(root.get("accessors"))?;
    array(root.get("skins"))?
        .iter()
        .map(|skin| {
            let skin = object(Some(skin))?;
            let joints = array(skin.get("joints"))?
                .iter()
                .map(|joint| required_index(Some(joint), nodes))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(GltfSkinPlan {
                name: skin
                    .get("name")
                    .map(|value| string(Some(value)).map(str::to_owned))
                    .transpose()?,
                joints,
                skeleton: optional_index(skin.get("skeleton"), nodes)?,
                inverse_bind_accessor: optional_index(skin.get("inverseBindMatrices"), accessors)?,
            })
        })
        .collect()
}

fn import_skinned_nodes(
    root: &Map<String, Value>,
) -> Result<Vec<GltfSkinnedNodePlan>, GltfImportError> {
    let meshes = array_len(root.get("meshes"))?;
    let skins = array_len(root.get("skins"))?;
    let mut bindings = Vec::new();
    for (node, value) in array(root.get("nodes"))?.iter().enumerate() {
        let value = object(Some(value))?;
        let Some(skin) = optional_index(value.get("skin"), skins)? else {
            continue;
        };
        bindings.push(GltfSkinnedNodePlan {
            node,
            mesh: required_index(value.get("mesh"), meshes)?,
            skin,
        });
    }
    Ok(bindings)
}

fn import_animations(root: &Map<String, Value>) -> Result<Vec<GltfAnimationPlan>, GltfImportError> {
    let accessors = array_len(root.get("accessors"))?;
    let nodes = array_len(root.get("nodes"))?;
    array(root.get("animations"))?
        .iter()
        .map(|animation| {
            let animation = object(Some(animation))?;
            let samplers = array(animation.get("samplers"))?
                .iter()
                .map(|sampler| {
                    let sampler = object(Some(sampler))?;
                    let interpolation = match sampler
                        .get("interpolation")
                        .and_then(Value::as_str)
                        .unwrap_or("LINEAR")
                    {
                        "LINEAR" => GltfInterpolation::Linear,
                        "STEP" => GltfInterpolation::Step,
                        "CUBICSPLINE" => GltfInterpolation::CubicSpline,
                        _ => return Err(GltfImportError::InvalidReference),
                    };
                    Ok(GltfAnimationSamplerPlan {
                        input_accessor: required_index(sampler.get("input"), accessors)?,
                        output_accessor: required_index(sampler.get("output"), accessors)?,
                        interpolation,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let channels = array(animation.get("channels"))?
                .iter()
                .map(|channel| {
                    let channel = object(Some(channel))?;
                    let target = object(channel.get("target"))?;
                    Ok(GltfAnimationChannelPlan {
                        sampler: required_index(channel.get("sampler"), samplers.len())?,
                        node: required_index(target.get("node"), nodes)?,
                        target: match string(target.get("path"))? {
                            "translation" => GltfAnimationTarget::Translation,
                            "rotation" => GltfAnimationTarget::Rotation,
                            "scale" => GltfAnimationTarget::Scale,
                            "weights" => GltfAnimationTarget::Weights,
                            _ => return Err(GltfImportError::InvalidReference),
                        },
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(GltfAnimationPlan {
                name: animation
                    .get("name")
                    .map(|value| string(Some(value)).map(str::to_owned))
                    .transpose()?,
                samplers,
                channels,
            })
        })
        .collect()
}

fn import_nodes(root: &Map<String, Value>) -> Result<Vec<GltfNodePlan>, GltfImportError> {
    let node_count = array_len(root.get("nodes"))?;
    let mesh_count = array_len(root.get("meshes"))?;
    let skin_count = array_len(root.get("skins"))?;
    array(root.get("nodes"))?
        .iter()
        .map(|node| {
            let node = object(Some(node))?;
            let children = array(node.get("children"))?
                .iter()
                .map(|child| required_index(Some(child), node_count))
                .collect::<Result<Vec<_>, _>>()?;
            let (local_matrix_milli, rest_trs) = parse_node_transform(node)?;
            Ok(GltfNodePlan {
                name: node
                    .get("name")
                    .map(|value| string(Some(value)).map(str::to_owned))
                    .transpose()?,
                mesh: optional_index(node.get("mesh"), mesh_count)?,
                skin: optional_index(node.get("skin"), skin_count)?,
                children,
                local_matrix_milli,
                rest_trs,
            })
        })
        .collect()
}

fn import_scenes(root: &Map<String, Value>) -> Result<Vec<GltfScenePlan>, GltfImportError> {
    let nodes = array_len(root.get("nodes"))?;
    array(root.get("scenes"))?
        .iter()
        .map(|scene| {
            let scene = object(Some(scene))?;
            Ok(GltfScenePlan {
                name: scene
                    .get("name")
                    .map(|value| string(Some(value)).map(str::to_owned))
                    .transpose()?,
                roots: array(scene.get("nodes"))?
                    .iter()
                    .map(|node| required_index(Some(node), nodes))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect()
}

fn parse_node_transform(
    node: &Map<String, Value>,
) -> Result<([i64; 16], Option<GltfNodeTrsPlan>), GltfImportError> {
    if let Some(matrix) = node.get("matrix") {
        let values = matrix
            .as_array()
            .filter(|values| values.len() == 16)
            .ok_or(GltfImportError::InvalidReference)?;
        let mut result = [0_i64; 16];
        for row in 0..4 {
            for column in 0..4 {
                result[row * 4 + column] = fixed_milli(values[column * 4 + row].as_f64())?;
            }
        }
        return Ok((result, None));
    }
    let translation = parse_f64_vector(node.get("translation"), [0.0, 0.0, 0.0])?;
    let scale = parse_f64_vector(node.get("scale"), [1.0, 1.0, 1.0])?;
    let rotation = parse_f64_vector4(node.get("rotation"), [0.0, 0.0, 0.0, 1.0])?;
    let length = rotation
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if length <= f64::EPSILON {
        return Err(GltfImportError::InvalidReference);
    }
    let [x, y, z, w] = rotation.map(|value| value / length);
    let rotation = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - z * w),
        2.0 * (x * z + y * w),
        2.0 * (x * y + z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - x * w),
        2.0 * (x * z - y * w),
        2.0 * (y * z + x * w),
        1.0 - 2.0 * (x * x + y * y),
    ];
    let mut result = [0_i64; 16];
    for row in 0..3 {
        for column in 0..3 {
            result[row * 4 + column] =
                fixed_milli(Some(rotation[row * 3 + column] * scale[column]))?;
        }
        result[row * 4 + 3] = fixed_milli(Some(translation[row]))?;
    }
    result[15] = 1_000;
    let mut translation_milli = [0; 3];
    let mut rotation_q30 = [0; 4];
    let mut scale_milli = [0; 3];
    for axis in 0..3 {
        translation_milli[axis] = fixed_scale(translation[axis], 1_000.0)?;
        scale_milli[axis] = fixed_scale(scale[axis], 1_000.0)?;
    }
    for (axis, value) in [x, y, z, w].into_iter().enumerate() {
        rotation_q30[axis] = fixed_scale(value, 1_000_000_000.0)?;
    }
    Ok((
        result,
        Some(GltfNodeTrsPlan {
            translation_milli,
            rotation_q30,
            scale_milli,
        }),
    ))
}

fn parse_f64_vector<const N: usize>(
    value: Option<&Value>,
    default: [f64; N],
) -> Result<[f64; N], GltfImportError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .filter(|values| values.len() == N)
        .ok_or(GltfImportError::InvalidReference)?;
    let mut result = [0.0; N];
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or(GltfImportError::InvalidReference)?;
    }
    Ok(result)
}

fn parse_f64_vector4(
    value: Option<&Value>,
    default: [f64; 4],
) -> Result<[f64; 4], GltfImportError> {
    parse_f64_vector(value, default)
}

fn fixed_milli(value: Option<f64>) -> Result<i64, GltfImportError> {
    let value = value
        .filter(|value| value.is_finite())
        .ok_or(GltfImportError::InvalidReference)?;
    let scaled = value * 1_000.0;
    if scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(GltfImportError::Capacity);
    }
    Ok(scaled.round() as i64)
}

fn fixed_scale(value: f64, scale: f64) -> Result<i64, GltfImportError> {
    let scaled = value * scale;
    if !scaled.is_finite() || scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(GltfImportError::Capacity);
    }
    Ok(scaled.round() as i64)
}

fn validate_extensions(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    const SUPPORTED: &[&str] = &[
        "KHR_materials_unlit",
        "KHR_texture_transform",
        "KHR_mesh_quantization",
    ];
    for extension in array(root.get("extensionsRequired"))? {
        let extension = string(Some(extension))?;
        if !SUPPORTED.contains(&extension) {
            return Err(GltfImportError::UnsupportedExtension);
        }
    }
    Ok(())
}

fn validate_config(config: GltfImportConfig) -> Result<(), GltfImportError> {
    if config.max_source_bytes == 0
        || config.max_json_bytes == 0
        || config.max_binary_bytes == 0
        || config.max_objects == 0
        || config.max_dependencies == 0
    {
        return Err(GltfImportError::InvalidConfig);
    }
    Ok(())
}

fn decode_glb(
    source: &[u8],
    config: GltfImportConfig,
) -> Result<(Vec<u8>, Option<Vec<u8>>), GltfImportError> {
    let mut offset = 12_usize;
    let mut json = None;
    let mut binary = None;
    while offset < source.len() {
        let chunk_header_end = offset
            .checked_add(8)
            .ok_or(GltfImportError::LengthMismatch)?;
        if chunk_header_end > source.len() {
            return Err(GltfImportError::InvalidGlbChunk);
        }
        let length = read_u32(source, offset)? as usize;
        let kind = read_u32(source, offset + 4)?;
        if !length.is_multiple_of(4) {
            return Err(GltfImportError::InvalidGlbChunk);
        }
        let end = chunk_header_end
            .checked_add(length)
            .filter(|end| *end <= source.len())
            .ok_or(GltfImportError::LengthMismatch)?;
        let bytes = &source[chunk_header_end..end];
        match kind {
            GLB_JSON_CHUNK if json.is_none() && binary.is_none() => {
                if bytes.len() > config.max_json_bytes {
                    return Err(GltfImportError::Capacity);
                }
                json = Some(trim_json_padding(bytes)?.to_vec());
            }
            GLB_BIN_CHUNK if json.is_some() && binary.is_none() => {
                if bytes.len() > config.max_binary_bytes {
                    return Err(GltfImportError::Capacity);
                }
                binary = Some(bytes.to_vec());
            }
            _ => return Err(GltfImportError::InvalidGlbChunk),
        }
        offset = end;
    }
    Ok((json.ok_or(GltfImportError::InvalidGlbChunk)?, binary))
}

fn trim_json_padding(bytes: &[u8]) -> Result<&[u8], GltfImportError> {
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .ok_or(GltfImportError::InvalidJson)?;
    if bytes[end..]
        .iter()
        .any(|byte| *byte != b' ' && *byte != b'\t')
    {
        return Err(GltfImportError::InvalidGlbChunk);
    }
    Ok(&bytes[..end])
}

fn validate_asset(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let asset = object(root.get("asset"))?;
    let version = string(asset.get("version"))?;
    if version != "2.0" {
        return Err(GltfImportError::UnsupportedVersion);
    }
    if asset
        .get("minVersion")
        .is_some_and(|value| value.as_str().is_none_or(|value| value > "2.0"))
    {
        return Err(GltfImportError::UnsupportedVersion);
    }
    Ok(())
}

fn summarize(
    root: &Map<String, Value>,
    config: GltfImportConfig,
) -> Result<GltfDocumentSummary, GltfImportError> {
    let meshes = array_len(root.get("meshes"))?;
    let mut primitives = 0_usize;
    for mesh in array(root.get("meshes"))? {
        primitives = primitives
            .checked_add(array_len(object(Some(mesh))?.get("primitives"))?)
            .ok_or(GltfImportError::Capacity)?;
    }
    let summary = GltfDocumentSummary {
        scenes: array_len(root.get("scenes"))?,
        nodes: array_len(root.get("nodes"))?,
        meshes,
        primitives,
        materials: array_len(root.get("materials"))?,
        textures: array_len(root.get("textures"))?,
        images: array_len(root.get("images"))?,
        skins: array_len(root.get("skins"))?,
        animations: array_len(root.get("animations"))?,
        accessors: array_len(root.get("accessors"))?,
        buffer_views: array_len(root.get("bufferViews"))?,
        buffers: array_len(root.get("buffers"))?,
    };
    let total = [
        summary.scenes,
        summary.nodes,
        summary.meshes,
        summary.primitives,
        summary.materials,
        summary.textures,
        summary.images,
        summary.skins,
        summary.animations,
        summary.accessors,
        summary.buffer_views,
        summary.buffers,
    ]
    .into_iter()
    .try_fold(0_usize, usize::checked_add)
    .ok_or(GltfImportError::Capacity)?;
    if total > config.max_objects {
        return Err(GltfImportError::Capacity);
    }
    Ok(summary)
}

fn collect_dependencies(
    root: &Map<String, Value>,
    config: GltfImportConfig,
) -> Result<Vec<GltfDependency>, GltfImportError> {
    let mut dependencies = Vec::new();
    let mut seen = BTreeSet::new();
    for value in array(root.get("buffers"))?
        .iter()
        .chain(array(root.get("images"))?.iter())
    {
        let Some(uri) = object(Some(value))?.get("uri") else {
            continue;
        };
        let uri = string(Some(uri))?;
        let embedded_data = uri.starts_with("data:");
        if !embedded_data && !safe_relative_uri(uri) {
            return Err(GltfImportError::UnsafeUri);
        }
        if seen.insert(uri.to_owned()) {
            if dependencies.len() == config.max_dependencies {
                return Err(GltfImportError::Capacity);
            }
            dependencies.push(GltfDependency {
                uri: uri.to_owned(),
                embedded_data,
            });
        }
    }
    Ok(dependencies)
}

fn validate_buffers(
    root: &Map<String, Value>,
    binary: Option<&[u8]>,
) -> Result<(), GltfImportError> {
    let mut binary_claimed = false;
    for buffer in array(root.get("buffers"))? {
        let buffer = object(Some(buffer))?;
        let byte_length = nonnegative_usize(buffer.get("byteLength"))?;
        match buffer.get("uri") {
            Some(uri) => {
                string(Some(uri))?;
            }
            None if !binary_claimed => {
                let binary = binary.ok_or(GltfImportError::MissingBinaryChunk)?;
                if byte_length > binary.len() {
                    return Err(GltfImportError::LengthMismatch);
                }
                binary_claimed = true;
            }
            None => return Err(GltfImportError::InvalidReference),
        }
    }
    Ok(())
}

fn validate_buffer_views(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let buffers = array(root.get("buffers"))?;
    for view in array(root.get("bufferViews"))? {
        let view = object(Some(view))?;
        let buffer = required_index(view.get("buffer"), buffers.len())?;
        let offset = optional_usize(view.get("byteOffset"))?.unwrap_or(0);
        let length = nonnegative_usize(view.get("byteLength"))?;
        let buffer_length = nonnegative_usize(object(Some(&buffers[buffer]))?.get("byteLength"))?;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > buffer_length)
        {
            return Err(GltfImportError::LengthMismatch);
        }
        if view.get("byteStride").is_some_and(|value| {
            value
                .as_u64()
                .is_none_or(|stride| !(4..=252).contains(&stride))
        }) {
            return Err(GltfImportError::InvalidReference);
        }
    }
    Ok(())
}

fn validate_accessors(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let views = array_len(root.get("bufferViews"))?;
    for accessor in array(root.get("accessors"))? {
        let accessor = object(Some(accessor))?;
        if let Some(view) = accessor.get("bufferView") {
            required_index(Some(view), views)?;
        } else if accessor.get("sparse").is_none() {
            return Err(GltfImportError::InvalidAccessor);
        }
        let component = accessor
            .get("componentType")
            .and_then(Value::as_u64)
            .ok_or(GltfImportError::InvalidAccessor)?;
        if !matches!(component, 5120 | 5121 | 5122 | 5123 | 5125 | 5126)
            || nonnegative_usize(accessor.get("count"))? == 0
            || !matches!(
                string(accessor.get("type"))?,
                "SCALAR" | "VEC2" | "VEC3" | "VEC4" | "MAT2" | "MAT3" | "MAT4"
            )
        {
            return Err(GltfImportError::InvalidAccessor);
        }
    }
    Ok(())
}

fn validate_meshes(root: &Map<String, Value>) -> Result<Vec<GltfPrimitivePlan>, GltfImportError> {
    let accessor_count = array_len(root.get("accessors"))?;
    let material_count = array_len(root.get("materials"))?;
    let mut plans = Vec::new();
    for (mesh_index, mesh) in array(root.get("meshes"))?.iter().enumerate() {
        let primitives = array(object(Some(mesh))?.get("primitives"))?;
        if primitives.is_empty() {
            return Err(GltfImportError::InvalidPrimitive);
        }
        for (primitive_index, primitive) in primitives.iter().enumerate() {
            let primitive = object(Some(primitive))?;
            let attributes = object(primitive.get("attributes"))?;
            if attributes.is_empty() || !attributes.contains_key("POSITION") {
                return Err(GltfImportError::InvalidPrimitive);
            }
            for accessor in attributes.values() {
                required_index(Some(accessor), accessor_count)?;
            }
            let indices = optional_index(primitive.get("indices"), accessor_count)?;
            let material = optional_index(primitive.get("material"), material_count)?;
            let mode = primitive.get("mode").map_or(Some(4), Value::as_u64);
            if mode.is_none_or(|mode| mode > 6) {
                return Err(GltfImportError::InvalidPrimitive);
            }
            let mut attribute_names = attributes.keys().cloned().collect::<Vec<_>>();
            attribute_names.sort();
            plans.push(GltfPrimitivePlan {
                mesh: mesh_index,
                primitive: primitive_index,
                attributes: attribute_names,
                position_accessor: required_index(attributes.get("POSITION"), accessor_count)?,
                normal_accessor: optional_index(attributes.get("NORMAL"), accessor_count)?,
                texcoord_accessor: optional_index(attributes.get("TEXCOORD_0"), accessor_count)?,
                joints_accessor: optional_index(attributes.get("JOINTS_0"), accessor_count)?,
                weights_accessor: optional_index(attributes.get("WEIGHTS_0"), accessor_count)?,
                indices,
                material,
                mode: mode.unwrap() as u8,
            });
        }
    }
    Ok(plans)
}

fn import_materials(root: &Map<String, Value>) -> Result<Vec<GltfMaterialPlan>, GltfImportError> {
    array(root.get("materials"))?
        .iter()
        .map(|material| {
            let material = object(Some(material))?;
            let alpha = match material.get("alphaMode").and_then(Value::as_str) {
                None | Some("OPAQUE") => GltfAlphaMode::Opaque,
                Some("BLEND") => GltfAlphaMode::Blend,
                Some("MASK") => {
                    let cutoff = material
                        .get("alphaCutoff")
                        .map_or(Some(0.5), Value::as_f64)
                        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                        .ok_or(GltfImportError::InvalidPrimitive)?;
                    GltfAlphaMode::Mask {
                        cutoff_q16: (cutoff * f64::from(u16::MAX)).round() as u16,
                    }
                }
                Some(_) => return Err(GltfImportError::InvalidPrimitive),
            };
            let double_sided = material
                .get("doubleSided")
                .map_or(Some(false), Value::as_bool)
                .ok_or(GltfImportError::InvalidPrimitive)?;
            let unlit = material
                .get("extensions")
                .and_then(Value::as_object)
                .is_some_and(|extensions| extensions.contains_key("KHR_materials_unlit"));
            let pbr = object_or_empty(material.get("pbrMetallicRoughness"))?;
            let base_color = pbr
                .get("baseColorFactor")
                .map(parse_color_factor)
                .transpose()?
                .unwrap_or([255, 255, 255, 255]);
            let metallic_q16 = parse_q16_factor(pbr.get("metallicFactor"), 1.0)?;
            let roughness_q16 = parse_q16_factor(pbr.get("roughnessFactor"), 1.0)?;
            let emissive_q16 = material
                .get("emissiveFactor")
                .map(parse_q16_vec3)
                .transpose()?
                .unwrap_or([0; 3]);
            let normal_scale_q16 = parse_q16_factor(
                object_or_empty(material.get("normalTexture"))?.get("scale"),
                1.0,
            )?;
            let occlusion_strength_q16 = parse_q16_factor(
                object_or_empty(material.get("occlusionTexture"))?.get("strength"),
                1.0,
            )?;
            let textures = [
                texture_index(pbr.get("baseColorTexture"))?,
                texture_index(pbr.get("metallicRoughnessTexture"))?,
                texture_index(material.get("normalTexture"))?,
                texture_index(material.get("occlusionTexture"))?,
                texture_index(material.get("emissiveTexture"))?,
            ];
            Ok(GltfMaterialPlan {
                name: material
                    .get("name")
                    .map(|value| string(Some(value)).map(str::to_owned))
                    .transpose()?,
                alpha,
                double_sided,
                unlit,
                base_color,
                metallic_q16,
                roughness_q16,
                emissive_q16,
                normal_scale_q16,
                occlusion_strength_q16,
                textures,
            })
        })
        .collect()
}

fn parse_q16_factor(value: Option<&Value>, default: f64) -> Result<u16, GltfImportError> {
    let value = value
        .map_or(Some(default), Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or(GltfImportError::InvalidPrimitive)?;
    Ok((value * f64::from(u16::MAX)).round() as u16)
}

fn parse_q16_vec3(value: &Value) -> Result<[u16; 3], GltfImportError> {
    let values = value
        .as_array()
        .filter(|values| values.len() == 3)
        .ok_or(GltfImportError::InvalidPrimitive)?;
    let mut result = [0; 3];
    for (index, value) in values.iter().enumerate() {
        result[index] = parse_q16_factor(Some(value), 0.0)?;
    }
    Ok(result)
}

fn texture_index(value: Option<&Value>) -> Result<Option<usize>, GltfImportError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let texture = object(Some(value))?;
    optional_index(texture.get("index"), usize::MAX)
}

pub fn materialize_meshes(
    plan: &GltfImportPlan,
    external_buffers: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<GltfMeshArtifact>, GltfImportError> {
    let root: Value =
        serde_json::from_slice(&plan.json_bytes).map_err(|_| GltfImportError::InvalidJson)?;
    let root = root
        .as_object()
        .ok_or(GltfImportError::InvalidJsonEnvelope)?;
    let buffers = materialize_buffers(root, plan.binary_chunk.as_deref(), external_buffers)?;
    let mut artifacts = Vec::with_capacity(plan.primitives.len());
    for primitive in &plan.primitives {
        if primitive.mode != 4 {
            return Err(GltfImportError::UnsupportedExtension);
        }
        let positions = read_vec3_f32(root, &buffers, primitive.position_accessor)?;
        let mut indices = match primitive.indices {
            Some(accessor) => read_indices(root, &buffers, accessor)?,
            None => (0..positions.len())
                .map(|index| u32::try_from(index).map_err(|_| GltfImportError::Capacity))
                .collect::<Result<Vec<_>, _>>()?,
        };
        if indices.len() % 3 != 0
            || indices
                .iter()
                .any(|index| *index as usize >= positions.len())
        {
            return Err(GltfImportError::InvalidPrimitive);
        }
        let normals = match primitive.normal_accessor {
            Some(accessor) => {
                let normals = read_vec3_f32(root, &buffers, accessor)?;
                if normals.len() != positions.len() {
                    return Err(GltfImportError::InvalidPrimitive);
                }
                normals
            }
            None => generate_normals(&positions, &indices),
        };
        let texcoords = match primitive.texcoord_accessor {
            Some(accessor) => {
                let texcoords = read_vec2_f32(root, &buffers, accessor)?;
                if texcoords.len() != positions.len() {
                    return Err(GltfImportError::InvalidPrimitive);
                }
                texcoords
            }
            None => vec![[0.0; 2]; positions.len()],
        };
        if primitive.joints_accessor.is_some() != primitive.weights_accessor.is_some() {
            return Err(GltfImportError::InvalidPrimitive);
        }
        let joints = primitive
            .joints_accessor
            .map(|accessor| read_vec4_joints(root, &buffers, accessor))
            .transpose()?;
        let weights = primitive
            .weights_accessor
            .map(|accessor| read_vec4_weights(root, &buffers, accessor))
            .transpose()?;
        if joints
            .as_ref()
            .is_some_and(|values| values.len() != positions.len())
            || weights
                .as_ref()
                .is_some_and(|values| values.len() != positions.len())
        {
            return Err(GltfImportError::InvalidPrimitive);
        }
        if indices.is_empty() {
            indices.shrink_to_fit();
        }
        artifacts.push(GltfMeshArtifact {
            mesh: primitive.mesh,
            primitive: primitive.primitive,
            positions,
            normals,
            texcoords,
            joints,
            weights,
            indices,
            material: primitive.material,
        });
    }
    Ok(artifacts)
}

pub fn materialize_images(
    plan: &GltfImportPlan,
    external_assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<GltfImageArtifact>, GltfImportError> {
    let root: Value =
        serde_json::from_slice(&plan.json_bytes).map_err(|_| GltfImportError::InvalidJson)?;
    let root = root
        .as_object()
        .ok_or(GltfImportError::InvalidJsonEnvelope)?;
    let buffers = materialize_buffers(root, plan.binary_chunk.as_deref(), external_assets)?;
    let views = array(root.get("bufferViews"))?;
    let mut artifacts = Vec::with_capacity(plan.images.len());
    for (index, image) in plan.images.iter().enumerate() {
        let encoded_bytes = if let Some(uri) = image.uri.as_deref() {
            if uri.starts_with("data:") {
                decode_data_uri(uri)?
            } else {
                external_assets
                    .get(uri)
                    .cloned()
                    .ok_or(GltfImportError::InvalidReference)?
            }
        } else {
            let view_index = image.buffer_view.ok_or(GltfImportError::InvalidReference)?;
            let view = object(views.get(view_index))?;
            let buffer_index = required_index(view.get("buffer"), buffers.len())?;
            let offset = optional_usize(view.get("byteOffset"))?.unwrap_or(0);
            let length = nonnegative_usize(view.get("byteLength"))?;
            let end = offset
                .checked_add(length)
                .ok_or(GltfImportError::Capacity)?;
            buffers
                .get(buffer_index)
                .and_then(|buffer| buffer.get(offset..end))
                .map(ToOwned::to_owned)
                .ok_or(GltfImportError::LengthMismatch)?
        };
        if encoded_bytes.is_empty() {
            return Err(GltfImportError::LengthMismatch);
        }
        artifacts.push(GltfImageArtifact {
            image: index,
            mime_type: image.mime_type.clone(),
            encoded_bytes,
        });
    }
    Ok(artifacts)
}

pub fn materialize_skins(
    plan: &GltfImportPlan,
    external_buffers: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<GltfSkinArtifact>, GltfImportError> {
    let root: Value =
        serde_json::from_slice(&plan.json_bytes).map_err(|_| GltfImportError::InvalidJson)?;
    let root = root
        .as_object()
        .ok_or(GltfImportError::InvalidJsonEnvelope)?;
    let buffers = materialize_buffers(root, plan.binary_chunk.as_deref(), external_buffers)?;
    plan.skins
        .iter()
        .enumerate()
        .map(|(skin, plan)| {
            let inverse_bind_matrices = match plan.inverse_bind_accessor {
                Some(accessor) => {
                    let matrices = read_mat4_f32(root, &buffers, accessor)?;
                    if matrices.len() != plan.joints.len() {
                        return Err(GltfImportError::InvalidAccessor);
                    }
                    matrices
                }
                None => vec![identity_matrix(); plan.joints.len()],
            };
            Ok(GltfSkinArtifact {
                skin,
                joints: plan.joints.clone(),
                skeleton: plan.skeleton,
                inverse_bind_matrices,
            })
        })
        .collect()
}

pub fn materialize_animations(
    plan: &GltfImportPlan,
    external_buffers: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<GltfAnimationArtifact>, GltfImportError> {
    let root: Value =
        serde_json::from_slice(&plan.json_bytes).map_err(|_| GltfImportError::InvalidJson)?;
    let root = root
        .as_object()
        .ok_or(GltfImportError::InvalidJsonEnvelope)?;
    let buffers = materialize_buffers(root, plan.binary_chunk.as_deref(), external_buffers)?;
    plan.animations
        .iter()
        .enumerate()
        .map(|(animation_index, animation)| {
            let mut channels = Vec::with_capacity(animation.channels.len());
            let mut duration_seconds = 0.0_f32;
            for channel in &animation.channels {
                let sampler = animation
                    .samplers
                    .get(channel.sampler)
                    .ok_or(GltfImportError::InvalidReference)?;
                let times_seconds = read_scalar_f32(root, &buffers, sampler.input_accessor)?;
                if times_seconds.is_empty()
                    || times_seconds[0] < 0.0
                    || times_seconds.windows(2).any(|times| times[0] >= times[1])
                {
                    return Err(GltfImportError::InvalidAccessor);
                }
                duration_seconds = duration_seconds.max(*times_seconds.last().unwrap());
                let values =
                    match channel.target {
                        GltfAnimationTarget::Translation | GltfAnimationTarget::Scale => {
                            GltfAnimationValues::Vec3(read_vec3_f32(
                                root,
                                &buffers,
                                sampler.output_accessor,
                            )?)
                        }
                        GltfAnimationTarget::Rotation => GltfAnimationValues::Vec4(read_vec4_f32(
                            root,
                            &buffers,
                            sampler.output_accessor,
                        )?),
                        GltfAnimationTarget::Weights => GltfAnimationValues::Scalar(
                            read_scalar_f32(root, &buffers, sampler.output_accessor)?,
                        ),
                    };
                let multiplier = if sampler.interpolation == GltfInterpolation::CubicSpline {
                    3
                } else {
                    1
                };
                let expected = times_seconds
                    .len()
                    .checked_mul(multiplier)
                    .ok_or(GltfImportError::Capacity)?;
                let value_count = match &values {
                    GltfAnimationValues::Scalar(values) => {
                        if values.len() % expected != 0 {
                            return Err(GltfImportError::InvalidAccessor);
                        }
                        values.len()
                    }
                    GltfAnimationValues::Vec3(values) => values.len(),
                    GltfAnimationValues::Vec4(values) => values.len(),
                };
                if value_count < expected
                    || (!matches!(&values, GltfAnimationValues::Scalar(_))
                        && value_count != expected)
                {
                    return Err(GltfImportError::InvalidAccessor);
                }
                channels.push(GltfAnimationChannelArtifact {
                    node: channel.node,
                    target: channel.target,
                    interpolation: sampler.interpolation,
                    times_seconds,
                    values,
                });
            }
            Ok(GltfAnimationArtifact {
                animation: animation_index,
                name: animation.name.clone(),
                channels,
                duration_seconds,
            })
        })
        .collect()
}

pub fn sample_animation_channel(
    channel: &GltfAnimationChannelArtifact,
    time_seconds: f32,
    looping: bool,
) -> Result<GltfSampledAnimationValue, GltfImportError> {
    if !time_seconds.is_finite() || channel.times_seconds.is_empty() {
        return Err(GltfImportError::InvalidAccessor);
    }
    let duration = *channel.times_seconds.last().unwrap();
    let time = if looping && duration > 0.0 {
        time_seconds.rem_euclid(duration)
    } else {
        time_seconds.clamp(channel.times_seconds[0], duration)
    };
    let upper = channel
        .times_seconds
        .partition_point(|sample| *sample <= time)
        .min(channel.times_seconds.len() - 1);
    let left = upper.saturating_sub(1);
    let right = if upper == 0 { 0 } else { upper };
    let span = channel.times_seconds[right] - channel.times_seconds[left];
    let amount = if span <= f32::EPSILON {
        0.0
    } else {
        (time - channel.times_seconds[left]) / span
    };
    match (&channel.values, channel.target) {
        (GltfAnimationValues::Vec3(values), GltfAnimationTarget::Translation) => {
            Ok(GltfSampledAnimationValue::Translation(sample_vec3(
                values,
                channel.interpolation,
                left,
                right,
                amount,
                span,
            )?))
        }
        (GltfAnimationValues::Vec3(values), GltfAnimationTarget::Scale) => {
            Ok(GltfSampledAnimationValue::Scale(sample_vec3(
                values,
                channel.interpolation,
                left,
                right,
                amount,
                span,
            )?))
        }
        (GltfAnimationValues::Vec4(values), GltfAnimationTarget::Rotation) => {
            let mut rotation =
                sample_vec4(values, channel.interpolation, left, right, amount, span)?;
            let length = rotation
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            if length <= f32::EPSILON {
                return Err(GltfImportError::InvalidAccessor);
            }
            rotation.iter_mut().for_each(|value| *value /= length);
            Ok(GltfSampledAnimationValue::Rotation(rotation))
        }
        (GltfAnimationValues::Scalar(values), GltfAnimationTarget::Weights) => {
            Ok(GltfSampledAnimationValue::Weights(sample_weights(
                values,
                channel.times_seconds.len(),
                channel.interpolation,
                left,
                right,
                amount,
                span,
            )?))
        }
        _ => Err(GltfImportError::InvalidAccessor),
    }
}

fn sample_vec3(
    values: &[[f32; 3]],
    interpolation: GltfInterpolation,
    left: usize,
    right: usize,
    amount: f32,
    span: f32,
) -> Result<[f32; 3], GltfImportError> {
    sample_array(values, interpolation, left, right, amount, span)
}

fn sample_vec4(
    values: &[[f32; 4]],
    interpolation: GltfInterpolation,
    left: usize,
    right: usize,
    amount: f32,
    span: f32,
) -> Result<[f32; 4], GltfImportError> {
    let mut right_values = values.to_vec();
    if interpolation != GltfInterpolation::CubicSpline {
        let dot = values[left]
            .iter()
            .zip(values[right])
            .map(|(left, right)| left * right)
            .sum::<f32>();
        if dot < 0.0 {
            right_values[right]
                .iter_mut()
                .for_each(|value| *value = -*value);
        }
    }
    sample_array(&right_values, interpolation, left, right, amount, span)
}

fn sample_array<const N: usize>(
    values: &[[f32; N]],
    interpolation: GltfInterpolation,
    left: usize,
    right: usize,
    amount: f32,
    span: f32,
) -> Result<[f32; N], GltfImportError> {
    match interpolation {
        GltfInterpolation::Step => values
            .get(left)
            .copied()
            .ok_or(GltfImportError::InvalidAccessor),
        GltfInterpolation::Linear => {
            let left = values.get(left).ok_or(GltfImportError::InvalidAccessor)?;
            let right = values.get(right).ok_or(GltfImportError::InvalidAccessor)?;
            Ok(std::array::from_fn(|axis| {
                left[axis] + (right[axis] - left[axis]) * amount
            }))
        }
        GltfInterpolation::CubicSpline => {
            let left_value = values
                .get(left * 3 + 1)
                .ok_or(GltfImportError::InvalidAccessor)?;
            let left_tangent = values
                .get(left * 3 + 2)
                .ok_or(GltfImportError::InvalidAccessor)?;
            let right_tangent = values
                .get(right * 3)
                .ok_or(GltfImportError::InvalidAccessor)?;
            let right_value = values
                .get(right * 3 + 1)
                .ok_or(GltfImportError::InvalidAccessor)?;
            Ok(std::array::from_fn(|axis| {
                hermite(
                    left_value[axis],
                    left_tangent[axis] * span,
                    right_value[axis],
                    right_tangent[axis] * span,
                    amount,
                )
            }))
        }
    }
}

fn sample_weights(
    values: &[f32],
    key_count: usize,
    interpolation: GltfInterpolation,
    left: usize,
    right: usize,
    amount: f32,
    span: f32,
) -> Result<Vec<f32>, GltfImportError> {
    let multiplier = if interpolation == GltfInterpolation::CubicSpline {
        3
    } else {
        1
    };
    let components = values
        .len()
        .checked_div(
            key_count
                .checked_mul(multiplier)
                .ok_or(GltfImportError::Capacity)?,
        )
        .filter(|components| *components != 0)
        .ok_or(GltfImportError::InvalidAccessor)?;
    (0..components)
        .map(|component| match interpolation {
            GltfInterpolation::Step => Ok(values[left * components + component]),
            GltfInterpolation::Linear => {
                let left = values[left * components + component];
                let right = values[right * components + component];
                Ok(left + (right - left) * amount)
            }
            GltfInterpolation::CubicSpline => {
                let block = components * 3;
                Ok(hermite(
                    values[left * block + components + component],
                    values[left * block + components * 2 + component] * span,
                    values[right * block + components + component],
                    values[right * block + component] * span,
                    amount,
                ))
            }
        })
        .collect()
}

fn hermite(left: f32, left_tangent: f32, right: f32, right_tangent: f32, t: f32) -> f32 {
    let squared = t * t;
    let cubed = squared * t;
    (2.0 * cubed - 3.0 * squared + 1.0) * left
        + (cubed - 2.0 * squared + t) * left_tangent
        + (-2.0 * cubed + 3.0 * squared) * right
        + (cubed - squared) * right_tangent
}

fn materialize_buffers(
    root: &Map<String, Value>,
    binary: Option<&[u8]>,
    external: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<Vec<u8>>, GltfImportError> {
    let mut materialized = Vec::new();
    let mut binary_consumed = false;
    for buffer in array(root.get("buffers"))? {
        let buffer = object(Some(buffer))?;
        let required = nonnegative_usize(buffer.get("byteLength"))?;
        let bytes = match buffer.get("uri") {
            Some(uri) => {
                let uri = string(Some(uri))?;
                if uri.starts_with("data:") {
                    decode_data_uri(uri)?
                } else {
                    external
                        .get(uri)
                        .cloned()
                        .ok_or(GltfImportError::InvalidReference)?
                }
            }
            None if !binary_consumed => {
                binary_consumed = true;
                binary
                    .map(ToOwned::to_owned)
                    .ok_or(GltfImportError::MissingBinaryChunk)?
            }
            None => return Err(GltfImportError::InvalidReference),
        };
        if bytes.len() < required {
            return Err(GltfImportError::LengthMismatch);
        }
        materialized.push(bytes);
    }
    Ok(materialized)
}

fn read_vec3_f32(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[f32; 3]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || string(accessor.get("type"))? != "VEC3"
        || accessor.contains_key("sparse")
    {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, 12)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let x = read_f32(bytes, start)?;
        let y = read_f32(bytes, start + 4)?;
        let z = read_f32(bytes, start + 8)?;
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err(GltfImportError::InvalidAccessor);
        }
        values.push([x, y, z]);
    }
    Ok(values)
}

fn read_scalar_f32(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<f32>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || string(accessor.get("type"))? != "SCALAR"
        || accessor.contains_key("sparse")
    {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, 4)?;
    (0..count)
        .map(|index| {
            let start = offset
                .checked_add(index.saturating_mul(stride))
                .ok_or(GltfImportError::Capacity)?;
            let value = read_f32(bytes, start)?;
            value
                .is_finite()
                .then_some(value)
                .ok_or(GltfImportError::InvalidAccessor)
        })
        .collect()
}

fn read_vec2_f32(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[f32; 2]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || string(accessor.get("type"))? != "VEC2"
        || accessor.contains_key("sparse")
    {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, 8)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let x = read_f32(bytes, start)?;
        let y = read_f32(bytes, start + 4)?;
        if !x.is_finite() || !y.is_finite() {
            return Err(GltfImportError::InvalidAccessor);
        }
        values.push([x, y]);
    }
    Ok(values)
}

fn read_vec4_f32(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[f32; 4]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || string(accessor.get("type"))? != "VEC4"
        || accessor.contains_key("sparse")
    {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, 16)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let value = [
            read_f32(bytes, start)?,
            read_f32(bytes, start + 4)?,
            read_f32(bytes, start + 8)?,
            read_f32(bytes, start + 12)?,
        ];
        if value.iter().any(|value| !value.is_finite()) {
            return Err(GltfImportError::InvalidAccessor);
        }
        values.push(value);
    }
    Ok(values)
}

fn read_vec4_joints(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[u16; 4]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if string(accessor.get("type"))? != "VEC4" || accessor.contains_key("sparse") {
        return Err(GltfImportError::InvalidAccessor);
    }
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or(GltfImportError::InvalidAccessor)?;
    let element_size = match component {
        5121 => 4,
        5123 => 8,
        _ => return Err(GltfImportError::InvalidAccessor),
    };
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, element_size)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let joints = match component {
            5121 => [
                u16::from(read_u8(bytes, start)?),
                u16::from(read_u8(bytes, start + 1)?),
                u16::from(read_u8(bytes, start + 2)?),
                u16::from(read_u8(bytes, start + 3)?),
            ],
            5123 => [
                read_u16(bytes, start)?,
                read_u16(bytes, start + 2)?,
                read_u16(bytes, start + 4)?,
                read_u16(bytes, start + 6)?,
            ],
            _ => unreachable!(),
        };
        values.push(joints);
    }
    Ok(values)
}

fn read_vec4_weights(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[f32; 4]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if string(accessor.get("type"))? != "VEC4" || accessor.contains_key("sparse") {
        return Err(GltfImportError::InvalidAccessor);
    }
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or(GltfImportError::InvalidAccessor)?;
    let normalized = accessor
        .get("normalized")
        .map_or(Some(false), Value::as_bool)
        .ok_or(GltfImportError::InvalidAccessor)?;
    let element_size = match (component, normalized) {
        (5121, true) => 4,
        (5123, true) => 8,
        (5126, false) => 16,
        _ => return Err(GltfImportError::InvalidAccessor),
    };
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, element_size)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let weights = match component {
            5121 => std::array::from_fn(|axis| f32::from(bytes[start + axis]) / f32::from(u8::MAX)),
            5123 => std::array::from_fn(|axis| {
                let offset = start + axis * 2;
                f32::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
                    / f32::from(u16::MAX)
            }),
            5126 => std::array::from_fn(|axis| {
                let offset = start + axis * 4;
                f32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            }),
            _ => unreachable!(),
        };
        if weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight < 0.0)
        {
            return Err(GltfImportError::InvalidAccessor);
        }
        values.push(weights);
    }
    Ok(values)
}

fn read_mat4_f32(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<[[f32; 4]; 4]>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || string(accessor.get("type"))? != "MAT4"
        || accessor.contains_key("sparse")
    {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, 64)?;
    let mut matrices = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        let mut matrix = [[0.0; 4]; 4];
        for (column, values) in matrix.iter_mut().enumerate() {
            for (row, value) in values.iter_mut().enumerate() {
                *value = read_f32(bytes, start + (column * 4 + row) * 4)?;
                if !value.is_finite() {
                    return Err(GltfImportError::InvalidAccessor);
                }
            }
        }
        matrices.push(matrix);
    }
    Ok(matrices)
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn read_indices(
    root: &Map<String, Value>,
    buffers: &[Vec<u8>],
    accessor_index: usize,
) -> Result<Vec<u32>, GltfImportError> {
    let accessors = array(root.get("accessors"))?;
    let accessor = object(accessors.get(accessor_index))?;
    if string(accessor.get("type"))? != "SCALAR" || accessor.contains_key("sparse") {
        return Err(GltfImportError::InvalidAccessor);
    }
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or(GltfImportError::InvalidAccessor)?;
    let element_size = match component {
        5121 => 1,
        5123 => 2,
        5125 => 4,
        _ => return Err(GltfImportError::InvalidAccessor),
    };
    let count = nonnegative_usize(accessor.get("count"))?;
    let (bytes, offset, stride) = accessor_bytes(root, buffers, accessor, element_size)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset
            .checked_add(index.saturating_mul(stride))
            .ok_or(GltfImportError::Capacity)?;
        values.push(match component {
            5121 => u32::from(*bytes.get(start).ok_or(GltfImportError::LengthMismatch)?),
            5123 => u32::from(u16::from_le_bytes(
                bytes
                    .get(start..start + 2)
                    .and_then(|value| value.try_into().ok())
                    .ok_or(GltfImportError::LengthMismatch)?,
            )),
            5125 => u32::from_le_bytes(
                bytes
                    .get(start..start + 4)
                    .and_then(|value| value.try_into().ok())
                    .ok_or(GltfImportError::LengthMismatch)?,
            ),
            _ => unreachable!(),
        });
    }
    Ok(values)
}

fn accessor_bytes<'a>(
    root: &Map<String, Value>,
    buffers: &'a [Vec<u8>],
    accessor: &Map<String, Value>,
    element_size: usize,
) -> Result<(&'a [u8], usize, usize), GltfImportError> {
    let views = array(root.get("bufferViews"))?;
    let view_index = required_index(accessor.get("bufferView"), views.len())?;
    let view = object(views.get(view_index))?;
    let buffer_index = required_index(view.get("buffer"), buffers.len())?;
    let view_offset = optional_usize(view.get("byteOffset"))?.unwrap_or(0);
    let accessor_offset = optional_usize(accessor.get("byteOffset"))?.unwrap_or(0);
    let offset = view_offset
        .checked_add(accessor_offset)
        .ok_or(GltfImportError::Capacity)?;
    let stride = optional_usize(view.get("byteStride"))?.unwrap_or(element_size);
    if stride < element_size {
        return Err(GltfImportError::InvalidAccessor);
    }
    let count = nonnegative_usize(accessor.get("count"))?;
    let end = offset
        .checked_add(count.saturating_sub(1).saturating_mul(stride))
        .and_then(|value| value.checked_add(element_size))
        .ok_or(GltfImportError::Capacity)?;
    let bytes = buffers
        .get(buffer_index)
        .ok_or(GltfImportError::InvalidReference)?;
    if end > bytes.len() {
        return Err(GltfImportError::LengthMismatch);
    }
    Ok((bytes, offset, stride))
}

fn generate_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for index in triangle {
            let target = &mut normals[*index as usize];
            target[0] += normal[0];
            target[1] += normal[1];
            target[2] += normal[2];
        }
    }
    for normal in &mut normals {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length > f32::EPSILON {
            normal[0] /= length;
            normal[1] /= length;
            normal[2] /= length;
        } else {
            *normal = [0.0, 1.0, 0.0];
        }
    }
    normals
}

fn parse_color_factor(value: &Value) -> Result<[u8; 4], GltfImportError> {
    let values = value
        .as_array()
        .filter(|values| values.len() == 4)
        .ok_or(GltfImportError::InvalidPrimitive)?;
    let mut color = [0_u8; 4];
    for (index, value) in values.iter().enumerate() {
        let value = value
            .as_f64()
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(GltfImportError::InvalidPrimitive)?;
        color[index] = (value * 255.0).round() as u8;
    }
    Ok(color)
}

fn object_or_empty(value: Option<&Value>) -> Result<&Map<String, Value>, GltfImportError> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value {
        Some(value) => value.as_object().ok_or(GltfImportError::InvalidJson),
        None => Ok(EMPTY.get_or_init(Map::new)),
    }
}

fn decode_data_uri(uri: &str) -> Result<Vec<u8>, GltfImportError> {
    let (metadata, payload) = uri
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .ok_or(GltfImportError::InvalidReference)?;
    if !metadata.ends_with(";base64") {
        return Err(GltfImportError::UnsupportedExtension);
    }
    decode_base64(payload)
}

fn data_uri_mime_type(uri: &str) -> Option<&str> {
    uri.strip_prefix("data:")?
        .split_once(',')?
        .0
        .split(';')
        .next()
        .filter(|mime| !mime.is_empty())
}

fn decode_base64(input: &str) -> Result<Vec<u8>, GltfImportError> {
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    let mut block = [0_u8; 4];
    let mut count = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        block[count] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(GltfImportError::InvalidReference),
        };
        count += 1;
        if count == 4 {
            if block[0] == 64 || block[1] == 64 || (block[2] == 64 && block[3] != 64) {
                return Err(GltfImportError::InvalidReference);
            }
            output.push((block[0] << 2) | (block[1] >> 4));
            if block[2] != 64 {
                output.push((block[1] << 4) | (block[2] >> 2));
            }
            if block[3] != 64 {
                output.push((block[2] << 6) | block[3]);
            }
            count = 0;
        }
    }
    if count != 0 {
        return Err(GltfImportError::InvalidReference);
    }
    Ok(output)
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, GltfImportError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(f32::from_le_bytes)
        .ok_or(GltfImportError::LengthMismatch)
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, GltfImportError> {
    bytes
        .get(offset)
        .copied()
        .ok_or(GltfImportError::LengthMismatch)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, GltfImportError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(GltfImportError::LengthMismatch)
}

fn validate_nodes(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let nodes = array(root.get("nodes"))?;
    let mesh_count = array_len(root.get("meshes"))?;
    let skin_count = array_len(root.get("skins"))?;
    let mut edges = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let node = object(Some(node))?;
        optional_index(node.get("mesh"), mesh_count)?;
        optional_index(node.get("skin"), skin_count)?;
        for child in array(node.get("children"))? {
            edges[index].push(required_index(Some(child), nodes.len())?);
        }
        if node.contains_key("matrix")
            && (node.contains_key("translation")
                || node.contains_key("rotation")
                || node.contains_key("scale"))
        {
            return Err(GltfImportError::InvalidReference);
        }
    }
    let mut state = vec![0_u8; nodes.len()];
    for node in 0..nodes.len() {
        visit_node(node, &edges, &mut state)?;
    }
    Ok(())
}

fn visit_node(node: usize, edges: &[Vec<usize>], state: &mut [u8]) -> Result<(), GltfImportError> {
    match state[node] {
        1 => return Err(GltfImportError::NodeCycle),
        2 => return Ok(()),
        _ => {}
    }
    state[node] = 1;
    for child in &edges[node] {
        visit_node(*child, edges, state)?;
    }
    state[node] = 2;
    Ok(())
}

fn validate_scenes(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let nodes = array_len(root.get("nodes"))?;
    for scene in array(root.get("scenes"))? {
        for node in array(object(Some(scene))?.get("nodes"))? {
            required_index(Some(node), nodes)?;
        }
    }
    Ok(())
}

fn validate_skins(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let nodes = array_len(root.get("nodes"))?;
    let accessors = array_len(root.get("accessors"))?;
    for skin in array(root.get("skins"))? {
        let skin = object(Some(skin))?;
        let joints = array(skin.get("joints"))?;
        if joints.is_empty() {
            return Err(GltfImportError::InvalidReference);
        }
        for joint in joints {
            required_index(Some(joint), nodes)?;
        }
        optional_index(skin.get("skeleton"), nodes)?;
        optional_index(skin.get("inverseBindMatrices"), accessors)?;
    }
    Ok(())
}

fn validate_animations(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let accessors = array_len(root.get("accessors"))?;
    let nodes = array_len(root.get("nodes"))?;
    for animation in array(root.get("animations"))? {
        let animation = object(Some(animation))?;
        let samplers = array(animation.get("samplers"))?;
        for sampler in samplers {
            let sampler = object(Some(sampler))?;
            required_index(sampler.get("input"), accessors)?;
            required_index(sampler.get("output"), accessors)?;
            if sampler.get("interpolation").is_some_and(|value| {
                value
                    .as_str()
                    .is_none_or(|value| !matches!(value, "LINEAR" | "STEP" | "CUBICSPLINE"))
            }) {
                return Err(GltfImportError::InvalidReference);
            }
        }
        for channel in array(animation.get("channels"))? {
            let channel = object(Some(channel))?;
            required_index(channel.get("sampler"), samplers.len())?;
            let target = object(channel.get("target"))?;
            optional_index(target.get("node"), nodes)?;
            if !matches!(
                string(target.get("path"))?,
                "translation" | "rotation" | "scale" | "weights"
            ) {
                return Err(GltfImportError::InvalidReference);
            }
        }
    }
    Ok(())
}

fn validate_texture_references(root: &Map<String, Value>) -> Result<(), GltfImportError> {
    let images = array_len(root.get("images"))?;
    let samplers = array_len(root.get("samplers"))?;
    let views = array_len(root.get("bufferViews"))?;
    for image in array(root.get("images"))? {
        let image = object(Some(image))?;
        if image.get("uri").is_none() {
            required_index(image.get("bufferView"), views)?;
            string(image.get("mimeType"))?;
        }
    }
    for texture in array(root.get("textures"))? {
        let texture = object(Some(texture))?;
        optional_index(texture.get("source"), images)?;
        optional_index(texture.get("sampler"), samplers)?;
    }
    Ok(())
}

fn safe_relative_uri(uri: &str) -> bool {
    !uri.is_empty()
        && !uri.starts_with('/')
        && !uri.starts_with('\\')
        && !uri.contains("://")
        && !uri.split(['/', '\\']).any(|segment| segment == "..")
}

fn array(value: Option<&Value>) -> Result<&[Value], GltfImportError> {
    match value {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or(GltfImportError::InvalidJson),
    }
}

fn array_len(value: Option<&Value>) -> Result<usize, GltfImportError> {
    array(value).map(<[Value]>::len)
}

fn object(value: Option<&Value>) -> Result<&Map<String, Value>, GltfImportError> {
    value
        .and_then(Value::as_object)
        .ok_or(GltfImportError::InvalidJson)
}

fn string(value: Option<&Value>) -> Result<&str, GltfImportError> {
    value
        .and_then(Value::as_str)
        .ok_or(GltfImportError::InvalidJson)
}

fn nonnegative_usize(value: Option<&Value>) -> Result<usize, GltfImportError> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(GltfImportError::InvalidReference)
}

fn optional_usize(value: Option<&Value>) -> Result<Option<usize>, GltfImportError> {
    value
        .map(|value| nonnegative_usize(Some(value)))
        .transpose()
}

fn required_index(value: Option<&Value>, length: usize) -> Result<usize, GltfImportError> {
    optional_index(value, length)?.ok_or(GltfImportError::InvalidReference)
}

fn optional_index(value: Option<&Value>, length: usize) -> Result<Option<usize>, GltfImportError> {
    let value = optional_usize(value)?;
    if value.is_some_and(|index| index >= length) {
        return Err(GltfImportError::InvalidReference);
    }
    Ok(value)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, GltfImportError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(GltfImportError::InvalidGlbHeader)
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(uri: &str) -> Vec<u8> {
        format!(
            r#"{{
                "asset": {{"version": "2.0"}},
                "buffers": [{{"uri": "{uri}", "byteLength": 12}}],
                "bufferViews": [{{"buffer": 0, "byteOffset": 0, "byteLength": 12}}],
                "accessors": [{{"bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3"}}],
                "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}]
            }}"#
        )
        .into_bytes()
    }

    #[test]
    fn json_import_builds_canonical_dependency_and_primitive_plan() {
        let imported = import(&document("mesh.bin"), GltfImportConfig::default()).unwrap();
        assert_eq!(imported.container, GltfContainer::Json);
        assert_eq!(imported.summary.meshes, 1);
        assert_eq!(imported.summary.primitives, 1);
        assert_eq!(
            imported.dependencies,
            vec![GltfDependency {
                uri: "mesh.bin".to_owned(),
                embedded_data: false,
            }]
        );
        assert_eq!(imported.primitives[0].position_accessor, 0);
        assert_eq!(imported.primitives[0].mode, 4);
    }

    #[test]
    fn traversal_uri_and_glb_length_mismatch_fail_before_plan_creation() {
        assert_eq!(
            import(&document("../secret.bin"), GltfImportConfig::default()),
            Err(GltfImportError::UnsafeUri)
        );
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&16_u32.to_le_bytes());
        assert_eq!(
            validate_container(&glb, GltfImportConfig::default()),
            Err(GltfImportError::LengthMismatch)
        );
    }
}
