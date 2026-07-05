use super::*;

impl Pipeline3D {
    pub fn clear_texture_bind_group_cache(&mut self) {
        self.main_texture_bind_groups.clear();
        self.terrain_texture_bind_groups.clear();
    }

    pub(super) fn create_material_sampler(
        device: &wgpu::Device,
        key: MaterialSamplerKey,
    ) -> wgpu::Sampler {
        let address_mode = match key.wrap_mode {
            crate::material::MATERIAL_WRAP_CLAMP => wgpu::AddressMode::ClampToEdge,
            crate::material::MATERIAL_WRAP_MIRROR => wgpu::AddressMode::MirrorRepeat,
            _ => wgpu::AddressMode::Repeat,
        };
        let filter = match key.filter_mode {
            crate::material::MATERIAL_FILTER_NEAREST => wgpu::FilterMode::Nearest,
            _ => wgpu::FilterMode::Linear,
        };
        let anisotropy_clamp = if filter == wgpu::FilterMode::Linear {
            8
        } else {
            1
        };
        device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_material_sampler"),
            address_mode_u: address_mode,
            address_mode_v: address_mode,
            address_mode_w: address_mode,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: filter,
            anisotropy_clamp,
            ..Default::default()
        })
    }

    pub(super) fn align_up(value: u32, alignment: u32) -> u32 {
        (value + alignment - 1) & !(alignment - 1)
    }

    pub(super) fn create_model_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        binding_size: u64,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(binding_size),
                }),
            }],
        })
    }

    pub(super) fn ensure_model_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.model_buffer_slot_count {
            return;
        }
        let new_count = needed.next_power_of_two().max(256);
        let aligned = Self::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.model_buffer,
            std::mem::size_of::<ModelUniform>() as u64,
            "voplay_mesh_model_bg",
        );
        self.model_buffer_slot_count = new_count;
    }

    pub(super) fn ensure_skinned_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.skinned_model_buffer_slot_count {
            return;
        }
        let new_count = needed.next_power_of_two().max(32);
        let aligned = Self::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.skinned_model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_skinned_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.skinned_model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.skinned_model_buffer,
            std::mem::size_of::<SkinnedModelUniform>() as u64,
            "voplay_mesh_skinned_model_bg",
        );
        self.skinned_model_buffer_slot_count = new_count;
    }

    pub(super) fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_instance_vb"),
            size: std::mem::size_of::<InstanceData>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    pub(super) fn valid_texture_id(textures: &TextureManager, texture_id: Option<u32>) -> u32 {
        texture_id
            .filter(|id| *id != 0 && textures.get(*id).is_some())
            .unwrap_or(0)
    }

    pub(super) fn resolve_main_texture_key(
        material: &MaterialOverride,
        mesh_material: &MeshMaterial,
        textures: &TextureManager,
    ) -> MainTextureKey {
        let albedo = if material.albedo_texture_id != 0
            && textures.get(material.albedo_texture_id).is_some()
        {
            material.albedo_texture_id
        } else {
            Self::valid_texture_id(textures, mesh_material.texture_id)
        };
        let normal = if material.normal_texture_id != 0
            && textures.get(material.normal_texture_id).is_some()
        {
            material.normal_texture_id
        } else {
            Self::valid_texture_id(textures, mesh_material.normal_texture_id)
        };
        let metallic_roughness = if material.metallic_roughness_texture_id != 0
            && textures
                .get(material.metallic_roughness_texture_id)
                .is_some()
        {
            material.metallic_roughness_texture_id
        } else {
            Self::valid_texture_id(textures, mesh_material.metallic_roughness_texture_id)
        };
        let emissive = if material.emissive_texture_id != 0
            && textures.get(material.emissive_texture_id).is_some()
        {
            material.emissive_texture_id
        } else {
            Self::valid_texture_id(textures, mesh_material.emissive_texture_id)
        };
        let toon_ramp = if material.toon_ramp_texture_id != 0
            && textures.get(material.toon_ramp_texture_id).is_some()
        {
            material.toon_ramp_texture_id
        } else {
            Self::valid_texture_id(textures, mesh_material.toon_ramp_texture_id)
        };
        let mask =
            if material.mask_texture_id != 0 && textures.get(material.mask_texture_id).is_some() {
                material.mask_texture_id
            } else {
                Self::valid_texture_id(textures, mesh_material.mask_texture_id)
            };
        MainTextureKey {
            albedo,
            normal,
            metallic_roughness,
            emissive,
            toon_ramp,
            mask,
            sampler: material.sampler_key(mesh_material.sampler),
        }
    }

    pub(super) fn texture_view_for_key<'a>(
        &'a self,
        textures: &'a TextureManager,
        texture_id: u32,
    ) -> &'a wgpu::TextureView {
        textures
            .get(texture_id)
            .map(|texture| &texture.view)
            .unwrap_or(&self.white_texture_view)
    }

    pub(super) fn valid_layer_texture_ids(
        textures: &TextureManager,
        texture_ids: &[Option<u32>; 4],
    ) -> [u32; 4] {
        std::array::from_fn(|index| Self::valid_texture_id(textures, texture_ids[index]))
    }

    pub(super) fn terrain_layer_normal_flags(
        textures: &TextureManager,
        material: &MeshMaterial,
    ) -> [f32; 4] {
        let normal_ids =
            Self::valid_layer_texture_ids(textures, &material.layer_normal_texture_ids);
        std::array::from_fn(|index| {
            if normal_ids[index] == 0 {
                0.0
            } else {
                material.layer_normal_scales[index].max(0.0)
            }
        })
    }

    pub(super) fn terrain_layer_mr_flags(
        textures: &TextureManager,
        material: &MeshMaterial,
    ) -> [f32; 4] {
        let mr_ids =
            Self::valid_layer_texture_ids(textures, &material.layer_metallic_roughness_texture_ids);
        std::array::from_fn(|index| if mr_ids[index] == 0 { 0.0 } else { 1.0 })
    }

    pub(super) fn terrain_material_key(tuning: TerrainMaterialTuning) -> [u32; 16] {
        let uniform = TerrainMaterialUniform::from_tuning(tuning);
        [
            uniform.params0[0].to_bits(),
            uniform.params0[1].to_bits(),
            uniform.params0[2].to_bits(),
            uniform.params0[3].to_bits(),
            uniform.params1[0].to_bits(),
            uniform.params1[1].to_bits(),
            uniform.params1[2].to_bits(),
            uniform.params1[3].to_bits(),
            uniform.params2[0].to_bits(),
            uniform.params2[1].to_bits(),
            uniform.params2[2].to_bits(),
            uniform.params2[3].to_bits(),
            uniform.params3[0].to_bits(),
            uniform.params3[1].to_bits(),
            uniform.params3[2].to_bits(),
            uniform.params3[3].to_bits(),
        ]
    }

    pub(super) fn sampler_for_key(&self, key: MaterialSamplerKey) -> &wgpu::Sampler {
        &self.material_samplers[key.sampler_index()]
    }

    pub(super) fn mesh_material_uniform_values(
        material: &MaterialOverride,
        mesh_material: &MeshMaterial,
        key: MainTextureKey,
    ) -> ([f32; 4], [f32; 4], [f32; 4], [f32; 4], [f32; 4]) {
        let override_emissive_texture_active =
            material.emissive_texture_id != 0 && key.emissive == material.emissive_texture_id;
        (
            material.mesh_material_params(
                mesh_material.uv_scales[0],
                mesh_material.roughness,
                mesh_material.metallic,
            ),
            material.emissive_color_value(
                mesh_material.emissive_factor,
                override_emissive_texture_active,
            ),
            key.texture_flags(material.normal_scale_value(mesh_material.normal_scale)),
            material.material_response_values(mesh_material),
            key.texture_flags2(),
        )
    }

    pub(super) fn create_main_texture_bind_group(
        &self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: MainTextureKey,
        shadow_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let albedo_view = self.texture_view_for_key(textures, key.albedo);
        let normal_view = self.texture_view_for_key(textures, key.normal);
        let metallic_roughness_view = self.texture_view_for_key(textures, key.metallic_roughness);
        let emissive_view = self.texture_view_for_key(textures, key.emissive);
        let toon_ramp_view = self.texture_view_for_key(textures, key.toon_ramp);
        let mask_view = self.texture_view_for_key(textures, key.mask);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_main_texture_bg"),
            layout: &self.main_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(self.sampler_for_key(key.sampler)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(metallic_roughness_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(emissive_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(toon_ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(mask_view),
                },
            ],
        })
    }

    pub(super) fn create_terrain_texture_bind_group(
        &self,
        device: &wgpu::Device,
        control_view: &wgpu::TextureView,
        control_sampler: &wgpu::Sampler,
        shadow_view: &wgpu::TextureView,
        albedo_layer_views: [&wgpu::TextureView; 4],
        normal_layer_views: [&wgpu::TextureView; 4],
        metallic_roughness_layer_views: [&wgpu::TextureView; 4],
        layer_sampler: &wgpu::Sampler,
        terrain_material_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let mut entries = Vec::with_capacity(17);
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(control_view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Sampler(control_sampler),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::TextureView(shadow_view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::Sampler(layer_sampler),
        });
        for (index, view) in albedo_layer_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 4 + index as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        for (index, view) in normal_layer_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 8 + index as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        for (index, view) in metallic_roughness_layer_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 12 + index as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 16,
            resource: terrain_material_buffer.as_entire_binding(),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_terrain_texture_bg"),
            layout: &self.terrain_texture_bind_group_layout,
            entries: &entries,
        })
    }

    pub(super) fn ensure_main_texture_bind_group(
        &mut self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: MainTextureKey,
        shadow_view: &wgpu::TextureView,
    ) {
        if self.main_texture_bind_groups.contains_key(&key) {
            return;
        }
        let bind_group = self.create_main_texture_bind_group(device, textures, key, shadow_view);
        self.main_texture_bind_groups.insert(key, bind_group);
    }

    pub(super) fn ensure_terrain_texture_bind_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &TextureManager,
        key: TerrainTextureKey,
        terrain_tuning: TerrainMaterialTuning,
        shadow_view: &wgpu::TextureView,
    ) {
        if self.terrain_texture_bind_groups.contains_key(&key) {
            return;
        }
        let control_view = self.texture_view_for_key(textures, key.control);
        let albedo_layer_views = key
            .albedo_layers
            .map(|id| self.texture_view_for_key(textures, id));
        let normal_layer_views = key
            .normal_layers
            .map(|id| self.texture_view_for_key(textures, id));
        let metallic_roughness_layer_views = key
            .metallic_roughness_layers
            .map(|id| self.texture_view_for_key(textures, id));
        let terrain_material_uniform = TerrainMaterialUniform::from_tuning(terrain_tuning);
        let terrain_material_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_terrain_material_uniform"),
            size: std::mem::size_of::<TerrainMaterialUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &terrain_material_buffer,
            0,
            bytemuck::bytes_of(&terrain_material_uniform),
        );
        let bind_group = self.create_terrain_texture_bind_group(
            device,
            control_view,
            &self.material_clamp_sampler,
            shadow_view,
            albedo_layer_views,
            normal_layer_views,
            metallic_roughness_layer_views,
            self.sampler_for_key(MaterialSamplerKey::REPEAT_LINEAR),
            &terrain_material_buffer,
        );
        self.terrain_texture_bind_groups.insert(
            key,
            TerrainBindGroupEntry {
                bind_group,
                _material_buffer: terrain_material_buffer,
            },
        );
    }

    /// Upload camera and light uniforms for this frame.
    pub fn set_camera_and_lights(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera3DUniform,
        lights: &LightUniform,
    ) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
        queue.write_buffer(&self.light_buffer, 0, bytemuck::bytes_of(lights));
    }
}
