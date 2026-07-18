use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitter;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitProfile {
    pub(crate) instance_count: u32,
    pub(crate) batch_hint: usize,
    pub(crate) skips: crate::render_world::RenderSkipStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshDrawStats {
    pub(crate) draw_calls: u32,
    pub(crate) batches: u32,
    pub(crate) instances: u32,
    pub(crate) triangles: u32,
    pub(crate) upload_bytes: u32,
    pub(crate) skips: crate::render_world::RenderSkipStats,
}

impl MeshSubmitter {
    pub(crate) fn prepare(
        draws: &[super::ModelDraw],
        models: &crate::model_loader::ModelManager,
        textures: &crate::texture::TextureManager,
    ) -> MeshSubmitProfile {
        let mut instance_count = 0u32;
        let mut batch_hint = 0usize;
        let mut skips = crate::render_world::RenderSkipStats::default();
        for draw in draws {
            let Some(model) = models.get(draw.model_id) else {
                skips.missing_models = skips.missing_models.saturating_add(1);
                continue;
            };
            for mesh in &model.meshes {
                let (missing_textures, fallback_paths) =
                    missing_mesh_texture_references(&draw.material, &mesh.material, textures);
                skips.missing_textures = skips.missing_textures.saturating_add(missing_textures);
                skips.fallback_paths = skips.fallback_paths.saturating_add(fallback_paths);
                if mesh.skinned || mesh.material.control_texture_id.is_some() {
                    continue;
                }
                instance_count = instance_count.saturating_add(1);
                batch_hint = batch_hint.saturating_add(1);
            }
        }
        MeshSubmitProfile {
            instance_count,
            batch_hint,
            skips,
        }
    }
}

fn missing_texture_reference(textures: &TextureManager, texture_id: Option<u32>) -> u32 {
    match texture_id.filter(|id| *id != 0) {
        Some(id) if textures.get(id).is_none() => 1,
        _ => 0,
    }
}

fn texture_reference_stats(
    textures: &TextureManager,
    preferred: u32,
    fallback: Option<u32>,
) -> (u32, u32) {
    let resolved = crate::texture::resolve_texture_reference(preferred, fallback, |id| {
        textures.get(id).is_some()
    });
    (resolved.missing_count, resolved.fallback_path_count)
}

fn missing_mesh_texture_references(
    material: &MaterialOverride,
    mesh: &MeshMaterial,
    textures: &TextureManager,
) -> (u32, u32) {
    if mesh.control_texture_id.is_some() {
        let mut missing = missing_texture_reference(textures, mesh.control_texture_id);
        for ids in [
            &mesh.layer_texture_ids,
            &mesh.layer_normal_texture_ids,
            &mesh.layer_metallic_roughness_texture_ids,
        ] {
            for id in ids {
                missing = missing.saturating_add(missing_texture_reference(textures, *id));
            }
        }
        return (missing, 0);
    }
    [
        texture_reference_stats(textures, material.albedo_texture_id, mesh.texture_id),
        texture_reference_stats(textures, material.normal_texture_id, mesh.normal_texture_id),
        texture_reference_stats(
            textures,
            material.metallic_roughness_texture_id,
            mesh.metallic_roughness_texture_id,
        ),
        texture_reference_stats(
            textures,
            material.emissive_texture_id,
            mesh.emissive_texture_id,
        ),
        texture_reference_stats(
            textures,
            material.toon_ramp_texture_id,
            mesh.toon_ramp_texture_id,
        ),
        texture_reference_stats(textures, material.mask_texture_id, mesh.mask_texture_id),
    ]
    .into_iter()
    .fold((0u32, 0u32), |(missing, fallbacks), item| {
        (
            missing.saturating_add(item.0),
            fallbacks.saturating_add(item.1),
        )
    })
}

impl MeshSubmitter {
    // Clippy exception — owner: voplay/render; reason: argument order follows the stable mesh
    // pass contract from frame resources through scene and target bindings; expiry: remove when
    // the render graph owns a typed mesh-pass descriptor shared by every backend.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit<'a>(
        owner: &'a mut Pipeline3D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[ModelDraw],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
    ) -> MeshDrawStats {
        if draws.is_empty() {
            return MeshDrawStats::default();
        }

        let mesh_submit_profile =
            super::mesh_submitter::MeshSubmitter::prepare(draws, models, textures);
        let mut stats = MeshDrawStats {
            skips: mesh_submit_profile.skips,
            ..MeshDrawStats::default()
        };
        let aligned_model_stride = Pipeline3D::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            owner.model_buffer_alignment,
        );
        let aligned_skinned_stride = Pipeline3D::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            owner.model_buffer_alignment,
        );

        let mut instance_batches: Vec<InstanceBatch> =
            Vec::with_capacity(mesh_submit_profile.batch_hint);
        let mut instance_batch_index: HashMap<InstanceBatchKey, usize> = HashMap::new();
        let mut static_slot: u32 = 0;
        let mut skinned_slot: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                let material_color = draw.material.base_color_multiplier();
                let base_color = [
                    mesh.material.base_color[0] * material_color[0],
                    mesh.material.base_color[1] * material_color[1],
                    mesh.material.base_color[2] * material_color[2],
                    mesh.material.base_color[3] * material_color[3],
                ];
                if mesh.skinned {
                    skinned_slot += 1;
                    continue;
                }
                if mesh.material.control_texture_id.is_some() {
                    static_slot += 1;
                    continue;
                }

                let texture_key =
                    Pipeline3D::resolve_main_texture_key(&draw.material, &mesh.material, textures);
                let key = InstanceBatchKey {
                    model_id: draw.model_id,
                    mesh_index,
                    textures: texture_key,
                };
                let batch_index = if let Some(index) = instance_batch_index.get(&key) {
                    *index
                } else {
                    let index = instance_batches.len();
                    instance_batches.push(InstanceBatch {
                        key,
                        instances: Vec::new(),
                    });
                    instance_batch_index.insert(key, index);
                    index
                };
                let mut model_uniform = draw.model_uniform;
                model_uniform.base_color = base_color;
                let (
                    material_params,
                    emissive_color,
                    texture_flags,
                    material_response,
                    texture_flags2,
                ) = Pipeline3D::mesh_material_uniform_values(
                    &draw.material,
                    &mesh.material,
                    texture_key,
                );
                model_uniform.material_params = material_params;
                model_uniform.emissive_color = emissive_color;
                model_uniform.texture_flags = texture_flags;
                model_uniform.material_response = material_response;
                model_uniform.texture_flags2 = texture_flags2;
                instance_batches[batch_index]
                    .instances
                    .push(InstanceData::from_uniform(&model_uniform));
            }
        }

        owner.ensure_model_capacity(device, static_slot);
        owner.ensure_skinned_capacity(device, skinned_slot);

        let mut instance_data = Vec::with_capacity(mesh_submit_profile.instance_count as usize);
        let mut instance_batch_draws = Vec::with_capacity(instance_batches.len());
        for batch in &instance_batches {
            let start = instance_data.len() as u32;
            instance_data.extend_from_slice(&batch.instances);
            instance_batch_draws.push(InstanceBatchDraw {
                key: batch.key,
                start,
                count: batch.instances.len() as u32,
            });
        }
        if !instance_data.is_empty() {
            owner.ensure_instance_capacity(device, instance_data.len() as u32);
            queue.write_buffer(
                &owner.instance_buffer,
                0,
                bytemuck::cast_slice(&instance_data),
            );
            stats.upload_bytes = instance_data
                .len()
                .saturating_mul(std::mem::size_of::<InstanceData>())
                .min(u32::MAX as usize) as u32;
        }

        static_slot = 0;
        skinned_slot = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for mesh in &gpu_model.meshes {
                let material_color = draw.material.base_color_multiplier();
                let base_color = [
                    mesh.material.base_color[0] * material_color[0],
                    mesh.material.base_color[1] * material_color[1],
                    mesh.material.base_color[2] * material_color[2],
                    mesh.material.base_color[3] * material_color[3],
                ];
                if mesh.skinned {
                    let texture_key = Pipeline3D::resolve_main_texture_key(
                        &draw.material,
                        &mesh.material,
                        textures,
                    );
                    let (
                        material_params,
                        emissive_color,
                        texture_flags,
                        material_response,
                        texture_flags2,
                    ) = Pipeline3D::mesh_material_uniform_values(
                        &draw.material,
                        &mesh.material,
                        texture_key,
                    );
                    let mut skinned_uniform = SkinnedModelUniform {
                        model: draw.model_uniform.model,
                        normal_matrix: draw.model_uniform.normal_matrix,
                        base_color,
                        material_params,
                        emissive_color,
                        texture_flags,
                        material_response,
                        texture_flags2,
                        joint_count: [0, 0, 0, 0],
                        joints: [[[0.0; 4]; 4]; MAX_JOINTS],
                    };
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    if joint_palette.len() > MAX_JOINTS {
                        stats.skips.incompatible_draws =
                            stats.skips.incompatible_draws.saturating_add(1);
                        continue;
                    }
                    skinned_uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        skinned_uniform.joints[index] = *matrix;
                    }
                    let offset = skinned_slot as u64 * aligned_skinned_stride as u64;
                    queue.write_buffer(
                        &owner.skinned_model_buffer,
                        offset,
                        bytemuck::bytes_of(&skinned_uniform),
                    );
                    skinned_slot += 1;
                } else {
                    if mesh.material.control_texture_id.is_none() {
                        continue;
                    }
                    let mut model_uniform = draw.model_uniform;
                    model_uniform.base_color = base_color;
                    model_uniform.material_params = [
                        draw.material.uv_scale_or(mesh.material.uv_scales[0]),
                        mesh.material.uv_scales[1],
                        mesh.material.uv_scales[2],
                        mesh.material.uv_scales[3],
                    ];
                    model_uniform.emissive_color =
                        Pipeline3D::terrain_layer_mr_flags(textures, &mesh.material);
                    model_uniform.texture_flags =
                        Pipeline3D::terrain_layer_normal_flags(textures, &mesh.material);
                    model_uniform.material_response = [1.0, 0.0, 1.0, 1.0];
                    model_uniform.texture_flags2 = [0.0, 0.0, 0.0, 0.0];
                    let offset = static_slot as u64 * aligned_model_stride as u64;
                    queue.write_buffer(
                        &owner.model_buffer,
                        offset,
                        bytemuck::bytes_of(&model_uniform),
                    );
                    static_slot += 1;
                }
            }
        }

        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for mesh in &gpu_model.meshes {
                if mesh.skinned {
                    let texture_key = Pipeline3D::resolve_main_texture_key(
                        &draw.material,
                        &mesh.material,
                        textures,
                    );
                    owner.ensure_main_texture_bind_group(
                        device,
                        textures,
                        texture_key,
                        shadow_view,
                    );
                    continue;
                }
                if let Some(control_id) = mesh.material.control_texture_id {
                    let control = Pipeline3D::valid_texture_id(textures, Some(control_id));
                    let terrain_key = TerrainTextureKey {
                        control,
                        albedo_layers: Pipeline3D::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_texture_ids,
                        ),
                        normal_layers: Pipeline3D::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_normal_texture_ids,
                        ),
                        metallic_roughness_layers: Pipeline3D::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_metallic_roughness_texture_ids,
                        ),
                        material: Pipeline3D::terrain_material_key(mesh.material.terrain_tuning),
                    };
                    owner.ensure_terrain_texture_bind_group(
                        device,
                        queue,
                        textures,
                        terrain_key,
                        mesh.material.terrain_tuning,
                        shadow_view,
                    );
                }
            }
        }
        for batch in &instance_batch_draws {
            owner.ensure_main_texture_bind_group(device, textures, batch.key.textures, shadow_view);
        }

        pass.set_bind_group(0, &owner.camera_bind_group, &[]);
        pass.set_bind_group(2, &owner.light_bind_group, &[]);

        static_slot = 0;
        skinned_slot = 0;

        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };

            for mesh in &gpu_model.meshes {
                if mesh.skinned {
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    if joint_palette.len() > MAX_JOINTS {
                        continue;
                    }
                    let dyn_offset = skinned_slot * aligned_skinned_stride;
                    pass.set_bind_group(1, &owner.skinned_model_bind_group, &[dyn_offset]);
                    skinned_slot += 1;

                    let texture_key = Pipeline3D::resolve_main_texture_key(
                        &draw.material,
                        &mesh.material,
                        textures,
                    );
                    let Some(main_texture_bind_group) =
                        owner.main_texture_bind_groups.get(&texture_key)
                    else {
                        stats.skips.missing_bind_groups =
                            stats.skips.missing_bind_groups.saturating_add(1);
                        continue;
                    };
                    let pipeline = match (texture_key.has_albedo(), aux_targets_enabled) {
                        (true, true) => &owner.pipeline_skinned_textured,
                        (true, false) => &owner.pipeline_skinned_textured_color,
                        (false, true) => &owner.pipeline_skinned_untextured,
                        (false, false) => &owner.pipeline_skinned_untextured_color,
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(3, main_texture_bind_group, &[]);
                } else {
                    if mesh.material.control_texture_id.is_none() {
                        continue;
                    }
                    let dyn_offset = static_slot * aligned_model_stride;
                    pass.set_bind_group(1, &owner.model_bind_group, &[dyn_offset]);
                    static_slot += 1;

                    if let Some(control_id) = mesh.material.control_texture_id {
                        pass.set_pipeline(if aux_targets_enabled {
                            &owner.pipeline_terrain_splat
                        } else {
                            &owner.pipeline_terrain_splat_color
                        });
                        let texture_key = Pipeline3D::valid_texture_id(textures, Some(control_id));
                        let layer_texture_ids = Pipeline3D::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_texture_ids,
                        );
                        let layer_normal_texture_ids = Pipeline3D::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_normal_texture_ids,
                        );
                        let layer_metallic_roughness_texture_ids =
                            Pipeline3D::valid_layer_texture_ids(
                                textures,
                                &mesh.material.layer_metallic_roughness_texture_ids,
                            );
                        let terrain_key = TerrainTextureKey {
                            control: texture_key,
                            albedo_layers: layer_texture_ids,
                            normal_layers: layer_normal_texture_ids,
                            metallic_roughness_layers: layer_metallic_roughness_texture_ids,
                            material: Pipeline3D::terrain_material_key(
                                mesh.material.terrain_tuning,
                            ),
                        };
                        let Some(terrain_texture_entry) =
                            owner.terrain_texture_bind_groups.get(&terrain_key)
                        else {
                            stats.skips.missing_bind_groups =
                                stats.skips.missing_bind_groups.saturating_add(1);
                            continue;
                        };
                        let terrain_texture_bind_group = &terrain_texture_entry.bind_group;
                        pass.set_bind_group(3, terrain_texture_bind_group, &[]);
                    }
                }

                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.draw_calls = stats.draw_calls.saturating_add(1);
                stats.batches = stats.batches.saturating_add(1);
                stats.instances = stats.instances.saturating_add(1);
                stats.triangles = stats.triangles.saturating_add(mesh.index_count / 3);
            }
        }

        if instance_batch_draws.is_empty() {
            return stats;
        }

        pass.set_bind_group(1, &owner.model_bind_group, &[0]);
        let instance_stride = std::mem::size_of::<InstanceData>() as u64;

        for batch in &instance_batch_draws {
            let gpu_model = match models.get(batch.key.model_id) {
                Some(m) => m,
                None => {
                    stats.skips.missing_models = stats.skips.missing_models.saturating_add(1);
                    continue;
                }
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                stats.skips.missing_meshes = stats.skips.missing_meshes.saturating_add(1);
                continue;
            };
            let texture_key = batch.key.textures;
            let Some(main_texture_bind_group) = owner.main_texture_bind_groups.get(&texture_key)
            else {
                stats.skips.missing_bind_groups = stats.skips.missing_bind_groups.saturating_add(1);
                continue;
            };
            let pipeline = match (texture_key.has_albedo(), aux_targets_enabled) {
                (true, true) => &owner.pipeline_instanced_textured,
                (true, false) => &owner.pipeline_instanced_textured_color,
                (false, true) => &owner.pipeline_instanced_untextured,
                (false, false) => &owner.pipeline_instanced_untextured_color,
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(3, main_texture_bind_group, &[]);

            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, owner.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
            stats.draw_calls = stats.draw_calls.saturating_add(1);
            stats.batches = stats.batches.saturating_add(1);
            stats.instances = stats.instances.saturating_add(batch.count);
            stats.triangles = stats
                .triangles
                .saturating_add((mesh.index_count / 3).saturating_mul(batch.count));
        }
        stats
    }
}
