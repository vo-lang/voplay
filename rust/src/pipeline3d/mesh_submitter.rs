use super::material_binder::MaterialBinder;
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitter;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitProfile {
    pub(crate) instance_count: u32,
    pub(crate) batch_hint: usize,
}

impl MeshSubmitter {
    pub(crate) fn prepare(
        draws: &[super::ModelDraw],
        models: &crate::model_loader::ModelManager,
    ) -> MeshSubmitProfile {
        let mut instance_count = 0u32;
        let mut batch_hint = 0usize;
        for draw in draws {
            let Some(model) = models.get(draw.model_id) else {
                continue;
            };
            for mesh in &model.meshes {
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
        }
    }
}

impl Pipeline3D {
    pub fn draw_models<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[ModelDraw],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
    ) {
        if draws.is_empty() {
            return;
        }

        let mesh_submit_profile = super::mesh_submitter::MeshSubmitter::prepare(draws, models);
        let skinned_slot_hint = super::skinned_submitter::SkinnedSubmitter::prepare(draws, models);
        let terrain_slot_hint = super::terrain_submitter::TerrainSubmitter::prepare(draws, models);

        let aligned_model_stride = Self::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        let aligned_skinned_stride = Self::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            self.model_buffer_alignment,
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
                    Self::resolve_main_texture_key(&draw.material, &mesh.material, textures);
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
                ) = Self::mesh_material_uniform_values(&draw.material, &mesh.material, texture_key);
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

        self.ensure_model_capacity(device, static_slot.max(terrain_slot_hint));
        self.ensure_skinned_capacity(device, skinned_slot.max(skinned_slot_hint));

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
            self.ensure_instance_capacity(device, instance_data.len() as u32);
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instance_data),
            );
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
                    let texture_key =
                        Self::resolve_main_texture_key(&draw.material, &mesh.material, textures);
                    let (
                        material_params,
                        emissive_color,
                        texture_flags,
                        material_response,
                        texture_flags2,
                    ) = Self::mesh_material_uniform_values(
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
                        continue;
                    }
                    skinned_uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        skinned_uniform.joints[index] = *matrix;
                    }
                    let offset = skinned_slot as u64 * aligned_skinned_stride as u64;
                    queue.write_buffer(
                        &self.skinned_model_buffer,
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
                        Self::terrain_layer_mr_flags(textures, &mesh.material);
                    model_uniform.texture_flags =
                        Self::terrain_layer_normal_flags(textures, &mesh.material);
                    model_uniform.material_response = [1.0, 0.0, 1.0, 1.0];
                    model_uniform.texture_flags2 = [0.0, 0.0, 0.0, 0.0];
                    let offset = static_slot as u64 * aligned_model_stride as u64;
                    queue.write_buffer(
                        &self.model_buffer,
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
                    let texture_key =
                        Self::resolve_main_texture_key(&draw.material, &mesh.material, textures);
                    self.ensure_main_texture_bind_group(device, textures, texture_key, shadow_view);
                    continue;
                }
                if let Some(control_id) = mesh.material.control_texture_id {
                    let control = Self::valid_texture_id(textures, Some(control_id));
                    let terrain_key = TerrainTextureKey {
                        control,
                        albedo_layers: Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_texture_ids,
                        ),
                        normal_layers: Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_normal_texture_ids,
                        ),
                        metallic_roughness_layers: Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_metallic_roughness_texture_ids,
                        ),
                        material: Self::terrain_material_key(mesh.material.terrain_tuning),
                    };
                    self.ensure_terrain_texture_bind_group(
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
            self.ensure_main_texture_bind_group(device, textures, batch.key.textures, shadow_view);
        }

        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);

        static_slot = 0;
        skinned_slot = 0;

        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };

            for mesh in &gpu_model.meshes {
                if mesh.skinned {
                    let dyn_offset = skinned_slot * aligned_skinned_stride;
                    MaterialBinder::bind_model_group(
                        pass,
                        &self.skinned_model_bind_group,
                        dyn_offset,
                    );
                    skinned_slot += 1;

                    let texture_key =
                        Self::resolve_main_texture_key(&draw.material, &mesh.material, textures);
                    let Some(main_texture_bind_group) =
                        self.main_texture_bind_groups.get(&texture_key)
                    else {
                        continue;
                    };
                    let pipeline = match (texture_key.has_albedo(), aux_targets_enabled) {
                        (true, true) => &self.pipeline_skinned_textured,
                        (true, false) => &self.pipeline_skinned_textured_color,
                        (false, true) => &self.pipeline_skinned_untextured,
                        (false, false) => &self.pipeline_skinned_untextured_color,
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                } else {
                    if mesh.material.control_texture_id.is_none() {
                        continue;
                    }
                    let dyn_offset = static_slot * aligned_model_stride;
                    MaterialBinder::bind_model_group(pass, &self.model_bind_group, dyn_offset);
                    static_slot += 1;

                    if let Some(control_id) = mesh.material.control_texture_id {
                        pass.set_pipeline(if aux_targets_enabled {
                            &self.pipeline_terrain_splat
                        } else {
                            &self.pipeline_terrain_splat_color
                        });
                        let texture_key = Self::valid_texture_id(textures, Some(control_id));
                        let layer_texture_ids = Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_texture_ids,
                        );
                        let layer_normal_texture_ids = Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_normal_texture_ids,
                        );
                        let layer_metallic_roughness_texture_ids = Self::valid_layer_texture_ids(
                            textures,
                            &mesh.material.layer_metallic_roughness_texture_ids,
                        );
                        let terrain_key = TerrainTextureKey {
                            control: texture_key,
                            albedo_layers: layer_texture_ids,
                            normal_layers: layer_normal_texture_ids,
                            metallic_roughness_layers: layer_metallic_roughness_texture_ids,
                            material: Self::terrain_material_key(mesh.material.terrain_tuning),
                        };
                        let Some(terrain_texture_entry) =
                            self.terrain_texture_bind_groups.get(&terrain_key)
                        else {
                            continue;
                        };
                        let terrain_texture_bind_group = &terrain_texture_entry.bind_group;
                        pass.set_bind_group(3, terrain_texture_bind_group, &[]);
                    }
                }

                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        if instance_batch_draws.is_empty() {
            return;
        }

        MaterialBinder::bind_model_group(pass, &self.model_bind_group, 0);
        let instance_stride = std::mem::size_of::<InstanceData>() as u64;

        for batch in &instance_batch_draws {
            let gpu_model = match models.get(batch.key.model_id) {
                Some(m) => m,
                None => continue,
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                continue;
            };
            let texture_key = batch.key.textures;
            let Some(main_texture_bind_group) = self.main_texture_bind_groups.get(&texture_key)
            else {
                continue;
            };
            let pipeline = match (texture_key.has_albedo(), aux_targets_enabled) {
                (true, true) => &self.pipeline_instanced_textured,
                (true, false) => &self.pipeline_instanced_textured_color,
                (false, true) => &self.pipeline_instanced_untextured,
                (false, false) => &self.pipeline_instanced_untextured_color,
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(3, &*main_texture_bind_group, &[]);

            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
        }
    }
}
