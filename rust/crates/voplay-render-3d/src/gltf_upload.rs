use std::collections::BTreeMap;

use voplay_import_gltf::{
    materialize_animations, materialize_meshes, materialize_skins, GltfAlphaMode,
    GltfAnimationArtifact, GltfImportError, GltfImportPlan, GltfSkinArtifact, GltfSkinnedNodePlan,
};

use crate::{
    AlphaMode, MaterialDescriptor, MeshDescriptor, MeshUpload, TextureUpload3d, WgpuSceneRenderer,
    WgpuSceneRendererError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportedPrimitive3d {
    pub mesh: u64,
    pub material: u64,
    pub source_mesh: usize,
    pub source_primitive: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportedNodePrimitive3d {
    pub node: usize,
    pub primitive: ImportedPrimitive3d,
    pub skin: Option<usize>,
    pub world_matrix_milli: [i64; 16],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportedGltf3d {
    pub namespace: u64,
    pub primitives: Vec<ImportedPrimitive3d>,
    pub materials: Vec<u64>,
    pub textures: Vec<u64>,
    pub skins: Vec<GltfSkinArtifact>,
    pub skinned_nodes: Vec<GltfSkinnedNodePlan>,
    pub animations: Vec<GltfAnimationArtifact>,
    pub scene_instances: Vec<ImportedNodePrimitive3d>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedGltfImage3d {
    pub image: usize,
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfUploadError {
    InvalidNamespace,
    DuplicateImage,
    MissingImage,
    MissingTexture,
    ArithmeticOverflow,
    Import(GltfImportError),
    Renderer(WgpuSceneRendererError),
}

impl From<GltfImportError> for GltfUploadError {
    fn from(value: GltfImportError) -> Self {
        Self::Import(value)
    }
}

impl From<WgpuSceneRendererError> for GltfUploadError {
    fn from(value: WgpuSceneRendererError) -> Self {
        Self::Renderer(value)
    }
}

pub fn upload_gltf_3d(
    renderer: &mut WgpuSceneRenderer,
    namespace: u64,
    plan: &GltfImportPlan,
    external_buffers: &BTreeMap<String, Vec<u8>>,
) -> Result<ImportedGltf3d, GltfUploadError> {
    upload_gltf_3d_with_images(renderer, namespace, plan, external_buffers, &[])
}

pub fn upload_gltf_3d_with_images(
    renderer: &mut WgpuSceneRenderer,
    namespace: u64,
    plan: &GltfImportPlan,
    external_buffers: &BTreeMap<String, Vec<u8>>,
    decoded_images: &[DecodedGltfImage3d],
) -> Result<ImportedGltf3d, GltfUploadError> {
    if namespace == 0 {
        return Err(GltfUploadError::InvalidNamespace);
    }
    let mut image_lookup = BTreeMap::new();
    for image in decoded_images {
        if image_lookup.insert(image.image, image).is_some() {
            return Err(GltfUploadError::DuplicateImage);
        }
    }
    let mut required_textures = BTreeMap::new();
    for material in &plan.materials {
        for (slot, texture) in material.textures.iter().enumerate() {
            let Some(texture) = texture else {
                continue;
            };
            let texture_plan = plan
                .textures
                .get(*texture)
                .ok_or(GltfUploadError::MissingTexture)?;
            let srgb = matches!(slot, 0 | 4);
            required_textures.insert((*texture, srgb), texture_plan.image);
        }
    }
    let mut texture_ids = Vec::with_capacity(required_textures.len());
    for ((texture, srgb), image) in required_textures {
        let image = image_lookup
            .get(&image)
            .ok_or(GltfUploadError::MissingImage)?;
        let id = gltf_texture_asset_id(namespace, texture, srgb);
        renderer.upload_texture(TextureUpload3d {
            id,
            width: image.width,
            height: image.height,
            rgba8: &image.rgba8,
            srgb,
        })?;
        texture_ids.push(id);
    }
    let mut material_ids = Vec::with_capacity(plan.materials.len().max(1));
    if plan.materials.is_empty() {
        let id = stable_asset_id(namespace, 1, 0, 0);
        renderer.register_material(MaterialDescriptor {
            id,
            pipeline_key: material_pipeline_key(false, false, false),
            alpha: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
            base_color: [255, 255, 255, 255],
            metallic_q16: 0,
            roughness_q16: u16::MAX,
            emissive_q16: [0; 3],
            normal_scale_q16: u16::MAX,
            occlusion_strength_q16: u16::MAX,
            textures: [0; 5],
        })?;
        material_ids.push(id);
    } else {
        for (index, material) in plan.materials.iter().enumerate() {
            let id = stable_asset_id(namespace, 1, index as u64, 0);
            let alpha = match material.alpha {
                GltfAlphaMode::Opaque => AlphaMode::Opaque,
                GltfAlphaMode::Mask { cutoff_q16 } => AlphaMode::Mask { cutoff_q16 },
                GltfAlphaMode::Blend => AlphaMode::Blend,
            };
            renderer.register_material(MaterialDescriptor {
                id,
                pipeline_key: material_pipeline_key(
                    matches!(alpha, AlphaMode::Blend),
                    material.double_sided,
                    material.unlit,
                ),
                alpha,
                double_sided: material.double_sided,
                unlit: material.unlit,
                base_color: material.base_color,
                metallic_q16: material.metallic_q16,
                roughness_q16: material.roughness_q16,
                emissive_q16: material.emissive_q16,
                normal_scale_q16: material.normal_scale_q16,
                occlusion_strength_q16: material.occlusion_strength_q16,
                textures: std::array::from_fn(|slot| {
                    material.textures[slot].map_or(0, |texture| {
                        gltf_texture_asset_id(namespace, texture, matches!(slot, 0 | 4))
                    })
                }),
            })?;
            material_ids.push(id);
        }
    }

    let artifacts = materialize_meshes(plan, external_buffers)?;
    let skins = materialize_skins(plan, external_buffers)?;
    let animations = materialize_animations(plan, external_buffers)?;
    let mut primitives = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let mesh = stable_asset_id(
            namespace,
            2,
            artifact.mesh as u64,
            artifact.primitive as u64,
        );
        let material = artifact
            .material
            .and_then(|index| material_ids.get(index).copied())
            .unwrap_or(material_ids[0]);
        let descriptor = MeshDescriptor {
            id: mesh,
            vertex_count: artifact.positions.len() as u32,
            index_count: artifact.indices.len() as u32,
            skinned: artifact.joints.is_some(),
        };
        renderer.upload_mesh(MeshUpload {
            descriptor,
            positions: &artifact.positions,
            normals: &artifact.normals,
            texcoords: &artifact.texcoords,
            joints: artifact.joints.as_deref(),
            weights: artifact.weights.as_deref(),
            indices: &artifact.indices,
        })?;
        primitives.push(ImportedPrimitive3d {
            mesh,
            material,
            source_mesh: artifact.mesh,
            source_primitive: artifact.primitive,
        });
    }
    let scene_instances = build_scene_instances(plan, &primitives)?;
    Ok(ImportedGltf3d {
        namespace,
        primitives,
        materials: material_ids,
        textures: texture_ids,
        skins,
        skinned_nodes: plan.skinned_nodes.clone(),
        animations,
        scene_instances,
    })
}

pub fn gltf_texture_asset_id(namespace: u64, texture: usize, srgb: bool) -> u64 {
    stable_asset_id(namespace, 3, texture as u64, u64::from(srgb))
}

fn stable_asset_id(namespace: u64, kind: u64, first: u64, second: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in [namespace, kind, first, second] {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash.max(1)
}

fn material_pipeline_key(blended: bool, double_sided: bool, unlit: bool) -> u64 {
    1 | (u64::from(blended) << 1) | (u64::from(double_sided) << 2) | (u64::from(unlit) << 3)
}

fn build_scene_instances(
    plan: &GltfImportPlan,
    primitives: &[ImportedPrimitive3d],
) -> Result<Vec<ImportedNodePrimitive3d>, GltfUploadError> {
    let roots = if let Some(scene) = plan
        .default_scene
        .or_else(|| (!plan.scenes.is_empty()).then_some(0))
    {
        plan.scenes
            .get(scene)
            .ok_or(GltfUploadError::Import(GltfImportError::InvalidReference))?
            .roots
            .clone()
    } else {
        let mut children = vec![false; plan.nodes.len()];
        for node in &plan.nodes {
            for child in &node.children {
                children[*child] = true;
            }
        }
        children
            .iter()
            .enumerate()
            .filter_map(|(node, child)| (!child).then_some(node))
            .collect()
    };
    let mut instances = Vec::new();
    for root in roots {
        collect_node_instances(
            plan,
            primitives,
            root,
            identity_matrix_milli(),
            &mut instances,
        )?;
    }
    Ok(instances)
}

fn collect_node_instances(
    plan: &GltfImportPlan,
    primitives: &[ImportedPrimitive3d],
    node_index: usize,
    parent: [i64; 16],
    instances: &mut Vec<ImportedNodePrimitive3d>,
) -> Result<(), GltfUploadError> {
    let node = plan
        .nodes
        .get(node_index)
        .ok_or(GltfUploadError::Import(GltfImportError::InvalidReference))?;
    let world = multiply_matrix_milli(parent, node.local_matrix_milli)?;
    if let Some(mesh) = node.mesh {
        instances.extend(
            primitives
                .iter()
                .filter(|primitive| primitive.source_mesh == mesh)
                .copied()
                .map(|primitive| ImportedNodePrimitive3d {
                    node: node_index,
                    primitive,
                    skin: node.skin,
                    world_matrix_milli: world,
                }),
        );
    }
    for child in &node.children {
        collect_node_instances(plan, primitives, *child, world, instances)?;
    }
    Ok(())
}

fn multiply_matrix_milli(left: [i64; 16], right: [i64; 16]) -> Result<[i64; 16], GltfUploadError> {
    let mut result = [0_i64; 16];
    for row in 0..4 {
        for column in 0..4 {
            let value = (0..4).fold(0_i128, |sum, index| {
                sum.saturating_add(
                    i128::from(left[row * 4 + index])
                        .saturating_mul(i128::from(right[index * 4 + column])),
                )
            }) / 1_000;
            result[row * 4 + column] =
                i64::try_from(value).map_err(|_| GltfUploadError::ArithmeticOverflow)?;
        }
    }
    Ok(result)
}

fn identity_matrix_milli() -> [i64; 16] {
    [
        1_000, 0, 0, 0, 0, 1_000, 0, 0, 0, 0, 1_000, 0, 0, 0, 0, 1_000,
    ]
}
