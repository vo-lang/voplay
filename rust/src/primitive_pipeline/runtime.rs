use super::*;

fn primitive_pass_keys_for_draw(
    draw: &PrimitiveDraw,
    models: &ModelManager,
    shadow_only: bool,
) -> Option<Vec<PrimitivePassBatchKey>> {
    if !primitive_participates_in_depth_pass(draw, shadow_only) {
        return Some(Vec::new());
    }
    let gpu_model = models.get(draw.model_id)?;
    let mut keys = Vec::new();
    for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
        if mesh.skinned {
            continue;
        }
        keys.push(PrimitivePassBatchKey {
            model_id: draw.model_id,
            mesh_index,
        });
    }
    Some(keys)
}

fn resident_pass_keys_stable(
    previous: &PrimitiveDraw,
    next: &PrimitiveDraw,
    models: &ModelManager,
) -> bool {
    for shadow_only in [false, true] {
        let Some(previous_keys) = primitive_pass_keys_for_draw(previous, models, shadow_only)
        else {
            return false;
        };
        let Some(next_keys) = primitive_pass_keys_for_draw(next, models, shadow_only) else {
            return false;
        };
        if previous_keys != next_keys {
            return false;
        }
    }
    true
}

fn resident_pass_batch_offset(
    instances: &[ResidentPrimitiveInstance],
    dirty_index: usize,
    key: PrimitivePassBatchKey,
    models: &ModelManager,
    shadow_only: bool,
) -> Option<u32> {
    let mut batch_offset = 0u32;
    for (index, instance) in instances.iter().enumerate() {
        let keys = primitive_pass_keys_for_draw(&instance.draw, models, shadow_only)?;
        for instance_key in keys {
            if instance_key == key {
                if index == dirty_index {
                    return Some(batch_offset);
                }
                batch_offset = batch_offset.saturating_add(1);
            }
        }
    }
    None
}

impl PrimitivePipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        receiver_mask_format: wgpu::TextureFormat,
        surface_props_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_primitive_mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/mesh.wgsl").into()),
        });
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_light_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let main_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay_primitive_texture_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_primitive_layout"),
            bind_group_layouts: &[
                &camera_bgl,
                &model_bgl,
                &light_bgl,
                &main_texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let translucent_depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let multisample = wgpu::MultisampleState {
            count: sample_count,
            ..wgpu::MultisampleState::default()
        };
        let vertex = wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_instanced_primitive"),
            buffers: &[MeshVertex::layout(), PrimitiveInstanceGpu::layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        };
        let pipeline_textured_opaque = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            true,
            true,
        );
        let pipeline_textured_opaque_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            true,
            false,
        );
        let pipeline_untextured_opaque = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            false,
            true,
        );
        let pipeline_untextured_opaque_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            false,
            false,
        );
        let pipeline_textured_cutout = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            true,
            true,
        );
        let pipeline_textured_cutout_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            true,
            false,
        );
        let pipeline_untextured_cutout = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            false,
            true,
        );
        let pipeline_untextured_cutout_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            false,
            false,
        );
        let pipeline_textured_translucent = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            translucent_depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            true,
            true,
        );
        let pipeline_textured_translucent_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            translucent_depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            true,
            false,
        );
        let pipeline_untextured_translucent = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            translucent_depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            false,
            true,
        );
        let pipeline_untextured_translucent_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex,
            surface_format,
            receiver_mask_format,
            surface_props_format,
            translucent_depth_stencil,
            multisample,
            PrimitiveRenderMode::Translucent,
            false,
            false,
        );

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_camera_ub"),
            size: std::mem::size_of::<Camera3DUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let model_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let model_size = align_up(std::mem::size_of::<ModelUniform>() as u32, model_alignment);
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_dummy_model_ub"),
            size: model_size as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_dummy_model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ModelUniform>() as u64),
                }),
            }],
        });
        queue.write_buffer(
            &model_buffer,
            0,
            bytemuck::bytes_of(&ModelUniform::zeroed()),
        );

        let instance_buffer_capacity: u32 = 1024;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_instance_vb"),
            size: std::mem::size_of::<PrimitiveInstanceGpu>() as u64
                * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_light_ub"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_light_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
        });
        let material_samplers = MATERIAL_SAMPLER_KEYS
            .iter()
            .map(|key| create_material_sampler(device, *key))
            .collect();
        let white_data = [255u8; 4];
        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_primitive_white_1x1"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &white_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_texture_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            pipeline_textured_opaque,
            pipeline_textured_opaque_color,
            pipeline_untextured_opaque,
            pipeline_untextured_opaque_color,
            pipeline_textured_cutout,
            pipeline_textured_cutout_color,
            pipeline_untextured_cutout,
            pipeline_untextured_cutout_color,
            pipeline_textured_translucent,
            pipeline_textured_translucent_color,
            pipeline_untextured_translucent,
            pipeline_untextured_translucent_color,
            camera_buffer,
            camera_bind_group,
            model_bind_group,
            instance_buffer,
            instance_buffer_capacity,
            light_buffer,
            light_bind_group,
            main_texture_bind_group_layout,
            material_samplers,
            white_texture_view,
            resident_chunks: HashMap::new(),
            object_chunks: HashMap::new(),
            rebuild_queue: Vec::new(),
            staging_instances: Vec::new(),
            rebuild_queue_peak: 0,
            last_resident_chunk_rebuilds: 0,
            last_resident_rebuild_policy: ResidentRebuildPolicy::default(),
            texture_bind_groups: HashMap::new(),
        }
    }

    pub fn clear_texture_bind_group_cache(&mut self) {
        self.texture_bind_groups.clear();
    }

    pub fn set_camera_and_lights(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera3DUniform,
        lights: &LightUniform,
    ) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
        queue.write_buffer(&self.light_buffer, 0, bytemuck::bytes_of(lights));
    }

    pub fn append_resident_depth_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
    ) {
        self.append_resident_pass_draws(chunk_refs, out, false);
    }

    pub fn append_resident_shadow_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
    ) {
        self.append_resident_pass_draws(chunk_refs, out, true);
    }

    pub fn append_resident_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
    ) -> u32 {
        let mut missing = 0u32;
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                missing = missing.saturating_add(1);
                continue;
            };
            out.extend(chunk.instances.iter().map(|instance| instance.draw));
        }
        missing
    }

    fn append_resident_pass_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
        shadow_only: bool,
    ) {
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for instance in &chunk.instances {
                if primitive_participates_in_depth_pass(&instance.draw, shadow_only) {
                    out.push(instance.draw);
                }
            }
        }
    }

    // Clippy exception — owner: voplay/render; reason: argument order follows the stable resident
    // scene-stream contract from GPU upload context through chunk identity and resource stores;
    // expiry: remove when the stream decoder emits a typed resident-chunk update context.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: &[PrimitiveObjectUpdate],
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let chunk_ref = PrimitiveChunkRef {
            scene_id,
            layer_id,
            chunk_id,
        };
        if let Some(previous) = self.resident_chunks.remove(&chunk_ref) {
            for instance in previous.instances {
                self.object_chunks.remove(&PrimitiveObjectKey {
                    scene_id,
                    layer_id,
                    object_id: instance.object_id,
                });
            }
        }
        for update in updates {
            let object_key = PrimitiveObjectKey {
                scene_id,
                layer_id,
                object_id: update.object_id,
            };
            if let Some(previous_chunk) = self.object_chunks.get(&object_key).copied() {
                if previous_chunk != chunk_ref {
                    self.remove_object_from_resident_chunk(
                        device,
                        queue,
                        object_key,
                        previous_chunk,
                        models,
                    );
                }
            }
        }
        let instances = updates
            .iter()
            .filter(|update| update.visible)
            .map(|update| ResidentPrimitiveInstance {
                object_id: update.object_id,
                draw: PrimitiveDraw::from_update(*update),
            })
            .collect::<Vec<_>>();
        for instance in &instances {
            self.object_chunks.insert(
                PrimitiveObjectKey {
                    scene_id,
                    layer_id,
                    object_id: instance.object_id,
                },
                chunk_ref,
            );
        }
        let depth_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, false);
        let shadow_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, true);
        self.resident_chunks.insert(
            chunk_ref,
            ResidentPrimitiveChunk {
                instances,
                depth_batches,
                shadow_batches,
            },
        );
    }

    pub fn upsert_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        update: PrimitiveObjectUpdate,
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let object_key = PrimitiveObjectKey {
            scene_id: update.scene_id,
            layer_id: update.layer_id,
            object_id: update.object_id,
        };
        let Some(chunk_ref) = self.object_chunks.get(&object_key).copied() else {
            return;
        };
        if !update.visible {
            self.remove_object_from_resident_chunk(device, queue, object_key, chunk_ref, models);
            return;
        }
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            self.object_chunks.remove(&object_key);
            return;
        };
        let mut requires_full_rebuild = true;
        let dirty_index = if let Some((index, instance)) = chunk
            .instances
            .iter_mut()
            .enumerate()
            .find(|(_, instance)| instance.object_id == update.object_id)
        {
            let next_draw = PrimitiveDraw::from_update(update);
            requires_full_rebuild = !resident_pass_keys_stable(&instance.draw, &next_draw, models);
            instance.draw = next_draw;
            index
        } else {
            chunk.instances.push(ResidentPrimitiveInstance {
                object_id: update.object_id,
                draw: PrimitiveDraw::from_update(update),
            });
            chunk.instances.len().saturating_sub(1)
        };
        self.queue_resident_rebuild(chunk_ref, dirty_index as u32, 1, requires_full_rebuild);
    }

    // Clippy exception — owner: voplay/render; reason: argument order matches the resident scene
    // lifecycle contract used by replace_chunk; expiry: remove when lifecycle commands carry a
    // typed resident-object context.
    #[allow(clippy::too_many_arguments)]
    pub fn destroy_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let object_key = PrimitiveObjectKey {
            scene_id,
            layer_id,
            object_id,
        };
        let Some(chunk_ref) = self.object_chunks.get(&object_key).copied() else {
            return;
        };
        self.remove_object_from_resident_chunk(device, queue, object_key, chunk_ref, models);
    }

    pub fn clear_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.resident_chunks.retain(|chunk_ref, _| {
            !(chunk_ref.scene_id == scene_id && chunk_ref.layer_id == layer_id)
        });
        self.object_chunks.retain(|object_key, _| {
            !(object_key.scene_id == scene_id && object_key.layer_id == layer_id)
        });
        self.rebuild_queue.retain(|range| {
            !(range.chunk_ref.scene_id == scene_id && range.chunk_ref.layer_id == layer_id)
        });
    }

    pub fn clear_scene(&mut self, scene_id: u32) {
        self.resident_chunks
            .retain(|chunk_ref, _| chunk_ref.scene_id != scene_id);
        self.object_chunks
            .retain(|object_key, _| object_key.scene_id != scene_id);
        self.rebuild_queue
            .retain(|range| range.chunk_ref.scene_id != scene_id);
    }

    // Clippy exception — owner: voplay/render; reason: argument order follows the stable primitive
    // pass contract across frame resources, scene data, target bindings, and filter; expiry:
    // remove when the render graph owns a typed primitive-pass descriptor shared by every backend.
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
        filter: PrimitiveRenderFilter,
    ) -> PrimitiveDrawStats {
        let mut stats = PrimitiveDrawStats::default();
        if draws.is_empty() && chunk_refs.is_empty() {
            return stats;
        }
        let mut batch_draws: Vec<PrimitiveBatchDraw> = Vec::new();
        let mut batches: Vec<PrimitiveBatch> = Vec::new();
        let mut batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
        let mut instance_count = 0u32;
        for draw in draws {
            if !primitive_draw_matches_filter(draw, filter) {
                stats.skips.filtered_draws = stats.skips.filtered_draws.saturating_add(1);
                continue;
            }
            if models.get(draw.model_id).is_none() {
                stats.skips.missing_models = stats.skips.missing_models.saturating_add(1);
                continue;
            }
            if filter == PrimitiveRenderFilter::Translucent {
                batch_index.clear();
            }
            let (pushed, texture_skips) = self.push_draw_batches(
                draw,
                models,
                textures,
                &mut batches,
                &mut batch_index,
                filter,
            );
            stats.skips.merge(texture_skips);
            if pushed == 0 {
                stats.skips.incompatible_draws = stats.skips.incompatible_draws.saturating_add(1);
            }
            instance_count = instance_count.saturating_add(pushed);
        }
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                stats.skips.missing_chunks = stats.skips.missing_chunks.saturating_add(1);
                continue;
            };
            for instance in &chunk.instances {
                if !primitive_draw_matches_filter(&instance.draw, filter) {
                    stats.skips.filtered_draws = stats.skips.filtered_draws.saturating_add(1);
                    continue;
                }
                if models.get(instance.draw.model_id).is_none() {
                    stats.skips.missing_models = stats.skips.missing_models.saturating_add(1);
                    continue;
                }
                if filter == PrimitiveRenderFilter::Translucent {
                    batch_index.clear();
                }
                let (pushed, texture_skips) = self.push_draw_batches(
                    &instance.draw,
                    models,
                    textures,
                    &mut batches,
                    &mut batch_index,
                    filter,
                );
                stats.skips.merge(texture_skips);
                if pushed == 0 {
                    stats.skips.incompatible_draws =
                        stats.skips.incompatible_draws.saturating_add(1);
                }
                instance_count = instance_count.saturating_add(pushed);
            }
        }
        if instance_count > 0 {
            self.ensure_instance_capacity(device, instance_count);
            let mut instance_data = Vec::with_capacity(instance_count as usize);
            batch_draws = Vec::with_capacity(batches.len());
            if filter != PrimitiveRenderFilter::Translucent {
                sort_primitive_batches(&mut batches);
            }
            for batch in &batches {
                let start = instance_data.len() as u32;
                instance_data.extend_from_slice(&batch.instances);
                batch_draws.push(PrimitiveBatchDraw {
                    key: batch.key,
                    start,
                    count: batch.instances.len() as u32,
                });
            }
            stats.prepared_batch_count = batch_draws.len().min(u32::MAX as usize) as u32;
            stats.upload_bytes = instance_data
                .len()
                .saturating_mul(std::mem::size_of::<PrimitiveInstanceGpu>())
                .min(u32::MAX as usize) as u32;
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instance_data),
            );
        }
        for batch in &batch_draws {
            self.ensure_texture_bind_group(device, textures, batch.key.textures, shadow_view);
        }

        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.model_bind_group, &[0]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);
        let instance_stride = std::mem::size_of::<PrimitiveInstanceGpu>() as u64;
        for batch in &batch_draws {
            let Some(gpu_model) = models.get(batch.key.model_id) else {
                stats.skips.missing_models = stats.skips.missing_models.saturating_add(1);
                continue;
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                stats.skips.missing_meshes = stats.skips.missing_meshes.saturating_add(1);
                continue;
            };
            let texture_key = batch.key.textures;
            let Some(texture_bind_group) = self.texture_bind_groups.get(&texture_key) else {
                stats.skips.missing_bind_groups = stats.skips.missing_bind_groups.saturating_add(1);
                continue;
            };
            pass.set_pipeline(self.pipeline_for_batch(batch.key, aux_targets_enabled));
            pass.set_bind_group(3, texture_bind_group, &[]);
            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
            stats.batch_count = stats.batch_count.saturating_add(1);
            stats.instance_count = stats.instance_count.saturating_add(batch.count);
            stats.triangle_count = stats
                .triangle_count
                .saturating_add((mesh.index_count / 3).saturating_mul(batch.count));
        }
        stats
    }

    pub fn has_water_surface(
        &self,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
    ) -> bool {
        if draws.iter().any(PrimitiveDraw::is_water_surface) {
            return true;
        }
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            if chunk
                .instances
                .iter()
                .any(|instance| instance.draw.is_water_surface())
            {
                return true;
            }
        }
        false
    }

    pub fn has_translucent_surface(
        &self,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
        models: &ModelManager,
        textures: &TextureManager,
    ) -> bool {
        draws.iter().any(|draw| {
            self.draw_has_render_mode(draw, models, textures, PrimitiveRenderMode::Translucent)
        }) || chunk_refs.iter().any(|chunk_ref| {
            self.resident_chunks
                .get(chunk_ref)
                .map(|chunk| {
                    chunk.instances.iter().any(|instance| {
                        self.draw_has_render_mode(
                            &instance.draw,
                            models,
                            textures,
                            PrimitiveRenderMode::Translucent,
                        )
                    })
                })
                .unwrap_or(false)
        })
    }

    fn pipeline_for_batch(
        &self,
        key: PrimitiveBatchKey,
        aux_targets_enabled: bool,
    ) -> &wgpu::RenderPipeline {
        match (key.textures.has_albedo(), key.mode, aux_targets_enabled) {
            (true, PrimitiveRenderMode::Opaque, true) => &self.pipeline_textured_opaque,
            (true, PrimitiveRenderMode::Opaque, false) => &self.pipeline_textured_opaque_color,
            (false, PrimitiveRenderMode::Opaque, true) => &self.pipeline_untextured_opaque,
            (false, PrimitiveRenderMode::Opaque, false) => &self.pipeline_untextured_opaque_color,
            (true, PrimitiveRenderMode::Cutout, true) => &self.pipeline_textured_cutout,
            (true, PrimitiveRenderMode::Cutout, false) => &self.pipeline_textured_cutout_color,
            (false, PrimitiveRenderMode::Cutout, true) => &self.pipeline_untextured_cutout,
            (false, PrimitiveRenderMode::Cutout, false) => &self.pipeline_untextured_cutout_color,
            (true, PrimitiveRenderMode::Translucent, true) => &self.pipeline_textured_translucent,
            (true, PrimitiveRenderMode::Translucent, false) => {
                &self.pipeline_textured_translucent_color
            }
            (false, PrimitiveRenderMode::Translucent, true) => {
                &self.pipeline_untextured_translucent
            }
            (false, PrimitiveRenderMode::Translucent, false) => {
                &self.pipeline_untextured_translucent_color
            }
        }
    }

    fn push_draw_batches(
        &self,
        draw: &PrimitiveDraw,
        models: &ModelManager,
        textures: &TextureManager,
        batches: &mut Vec<PrimitiveBatch>,
        batch_index: &mut HashMap<PrimitiveBatchKey, usize>,
        filter: PrimitiveRenderFilter,
    ) -> (u32, RenderSkipStats) {
        if !primitive_draw_matches_filter(draw, filter) {
            return (0, RenderSkipStats::default());
        }
        let Some(gpu_model) = models.get(draw.model_id) else {
            return (0, RenderSkipStats::default());
        };
        let mut pushed = 0u32;
        let mut texture_skips = RenderSkipStats::default();
        for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
            if mesh.skinned || mesh.material.control_texture_id.is_some() {
                continue;
            }
            let (texture_key, resolved_skips) =
                resolve_texture_key(&draw.material, &mesh.material, textures);
            texture_skips.merge(resolved_skips);
            let mut uniform = draw.model_uniform;
            uniform.base_color = combined_base_color(&draw.material, &mesh.material.base_color);
            let (material_params, emissive_color, texture_flags) =
                mesh_uniform_values(&draw.material, &mesh.material, texture_key);
            uniform.material_params = material_params;
            uniform.emissive_color = emissive_color;
            uniform.texture_flags = texture_flags;
            let key = PrimitiveBatchKey {
                model_id: draw.model_id,
                mesh_index,
                textures: texture_key,
                mode: primitive_render_mode(draw, uniform.base_color),
            };
            if !primitive_mode_matches_filter(key.mode, filter) {
                continue;
            }
            let index = if let Some(index) = batch_index.get(&key) {
                *index
            } else {
                let index = batches.len();
                batches.push(PrimitiveBatch {
                    key,
                    instances: Vec::new(),
                });
                batch_index.insert(key, index);
                index
            };
            batches[index]
                .instances
                .push(PrimitiveInstanceGpu::from_draw(draw, &uniform));
            pushed += 1;
        }
        (pushed, texture_skips)
    }

    fn draw_has_render_mode(
        &self,
        draw: &PrimitiveDraw,
        models: &ModelManager,
        _textures: &TextureManager,
        mode: PrimitiveRenderMode,
    ) -> bool {
        if !primitive_draw_matches_filter(draw, PrimitiveRenderFilter::Translucent) {
            return false;
        }
        let Some(gpu_model) = models.get(draw.model_id) else {
            return false;
        };
        gpu_model.meshes.iter().any(|mesh| {
            if mesh.skinned || mesh.material.control_texture_id.is_some() {
                return false;
            }
            let uniform_base_color = combined_base_color(&draw.material, &mesh.material.base_color);
            primitive_render_mode(draw, uniform_base_color) == mode
        })
    }

    fn remove_object_from_resident_chunk(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        object_key: PrimitiveObjectKey,
        chunk_ref: PrimitiveChunkRef,
        _models: &ModelManager,
    ) {
        self.object_chunks.remove(&object_key);
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            return;
        };
        let Some(dirty_index) = chunk
            .instances
            .iter()
            .position(|instance| instance.object_id == object_key.object_id)
        else {
            return;
        };
        chunk.instances.remove(dirty_index);
        self.queue_resident_rebuild(chunk_ref, dirty_index as u32, 1, true);
    }

    fn queue_resident_rebuild(
        &mut self,
        chunk_ref: PrimitiveChunkRef,
        dirty_start: u32,
        dirty_count: u32,
        requires_full_rebuild: bool,
    ) {
        let dirty_count = dirty_count.max(1);
        if let Some(range) = self
            .rebuild_queue
            .iter_mut()
            .find(|range| range.chunk_ref == chunk_ref)
        {
            let start = range.dirty_start.min(dirty_start);
            let end = range
                .dirty_start
                .saturating_add(range.dirty_count)
                .max(dirty_start.saturating_add(dirty_count));
            range.dirty_start = start;
            range.dirty_count = end.saturating_sub(start).max(1);
            range.requires_full_rebuild |= requires_full_rebuild;
        } else {
            self.rebuild_queue.push(PrimitiveChunkDirtyRange {
                chunk_ref,
                dirty_start,
                dirty_count,
                requires_full_rebuild,
            });
        }
        self.rebuild_queue_peak = self
            .rebuild_queue_peak
            .max(self.rebuild_queue.len().min(u32::MAX as usize) as u32);
    }

    pub fn flush_resident_rebuild_queue(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        models: &ModelManager,
    ) -> u32 {
        if self.rebuild_queue.is_empty() {
            self.last_resident_rebuild_policy = ResidentRebuildPolicy::default();
            return 0;
        }
        let pending = std::mem::take(&mut self.rebuild_queue);
        let queue_peak = self.rebuild_queue_peak;
        self.rebuild_queue_peak = 0;
        let mut resident_chunk_rebuilds = 0u32;
        let mut partial_uploads = 0u32;
        let mut dirty_upload_bytes = 0u64;
        for range in pending {
            let dirty_upload_range =
                range.dirty_start..range.dirty_start.saturating_add(range.dirty_count);
            let dirty_upload_count = dirty_upload_range
                .end
                .saturating_sub(dirty_upload_range.start);
            let _queue_peak = queue_peak;
            if !range.requires_full_rebuild {
                if let Some(uploaded_bytes) = self.upload_resident_dirty_range(
                    queue,
                    range.chunk_ref,
                    range.dirty_start,
                    dirty_upload_count,
                    models,
                ) {
                    dirty_upload_bytes = dirty_upload_bytes.saturating_add(uploaded_bytes);
                    partial_uploads = partial_uploads.saturating_add(1);
                    continue;
                }
            }
            dirty_upload_bytes = dirty_upload_bytes.saturating_add(
                u64::from(dirty_upload_count)
                    .saturating_mul(std::mem::size_of::<PrimitivePassInstanceGpu>() as u64),
            );
            if self.rebuild_resident_chunk_full(device, queue, range.chunk_ref, models) {
                resident_chunk_rebuilds = resident_chunk_rebuilds.saturating_add(1);
            }
        }
        self.last_resident_chunk_rebuilds = resident_chunk_rebuilds;
        let rebuild_reason = match (partial_uploads > 0, resident_chunk_rebuilds > 0) {
            (true, false) => "dirty-range-partial-upload",
            (true, true) => "dirty-range-mixed-partial-full",
            (false, true) => "dirty-range-structural-full-rebuild",
            (false, false) => "dirty-range-noop",
        };
        self.last_resident_rebuild_policy = ResidentRebuildPolicy {
            dirty_upload_bytes,
            full_rebuild_count: resident_chunk_rebuilds,
            rebuild_reason,
        };
        self.last_resident_chunk_rebuilds
    }

    fn upload_resident_dirty_range(
        &self,
        queue: &wgpu::Queue,
        chunk_ref: PrimitiveChunkRef,
        dirty_start: u32,
        dirty_count: u32,
        models: &ModelManager,
    ) -> Option<u64> {
        if dirty_count != 1 {
            return None;
        }
        let dirty_index = dirty_start as usize;
        let chunk = self.resident_chunks.get(&chunk_ref)?;
        let instance = chunk.instances.get(dirty_index)?;
        let mut uploaded_bytes = 0u64;
        for shadow_only in [false, true] {
            let batches = if shadow_only {
                &chunk.shadow_batches
            } else {
                &chunk.depth_batches
            };
            for key in primitive_pass_keys_for_draw(&instance.draw, models, shadow_only)? {
                let batch = batches.iter().find(|batch| batch.key == key)?;
                let batch_offset = resident_pass_batch_offset(
                    &chunk.instances,
                    dirty_index,
                    key,
                    models,
                    shadow_only,
                )?;
                if batch_offset >= batch.count {
                    return None;
                }
                let gpu_instance =
                    PrimitivePassInstanceGpu::from_model(instance.draw.model_uniform.model);
                let byte_offset = u64::from(batch_offset)
                    .saturating_mul(std::mem::size_of::<PrimitivePassInstanceGpu>() as u64);
                queue.write_buffer(
                    &batch.buffer,
                    byte_offset,
                    bytemuck::bytes_of(&gpu_instance),
                );
                uploaded_bytes = uploaded_bytes
                    .saturating_add(std::mem::size_of::<PrimitivePassInstanceGpu>() as u64);
            }
        }
        Some(uploaded_bytes)
    }

    fn rebuild_resident_chunk_full(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunk_ref: PrimitiveChunkRef,
        models: &ModelManager,
    ) -> bool {
        self.staging_instances.clear();
        let Some(chunk) = self.resident_chunks.get(&chunk_ref) else {
            return false;
        };
        self.staging_instances
            .extend(
                chunk
                    .instances
                    .iter()
                    .map(|instance| ResidentPrimitiveInstance {
                        object_id: instance.object_id,
                        draw: instance.draw,
                    }),
            );
        let instances = self.staging_instances.clone();
        let depth_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, false);
        let shadow_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, true);
        if instances.is_empty() {
            self.resident_chunks.remove(&chunk_ref);
        } else if let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) {
            chunk.depth_batches = depth_batches;
            chunk.shadow_batches = shadow_batches;
        }
        true
    }

    fn build_resident_pass_batches(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[ResidentPrimitiveInstance],
        models: &ModelManager,
        shadow_only: bool,
    ) -> Vec<ResidentPrimitivePassBatch> {
        let mut batches: HashMap<PrimitivePassBatchKey, Vec<PrimitivePassInstanceGpu>> =
            HashMap::new();
        for instance in instances {
            if !primitive_participates_in_depth_pass(&instance.draw, shadow_only) {
                continue;
            }
            let Some(gpu_model) = models.get(instance.draw.model_id) else {
                continue;
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                if mesh.skinned {
                    continue;
                }
                let key = PrimitivePassBatchKey {
                    model_id: instance.draw.model_id,
                    mesh_index,
                };
                batches
                    .entry(key)
                    .or_default()
                    .push(PrimitivePassInstanceGpu::from_model(
                        instance.draw.model_uniform.model,
                    ));
            }
        }
        batches
            .into_iter()
            .filter_map(|(key, instances)| {
                if instances.is_empty() {
                    return None;
                }
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("voplay_primitive_chunk_pass_instance_vb"),
                    size: std::mem::size_of::<PrimitivePassInstanceGpu>() as u64
                        * instances.len() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&instances));
                Some(ResidentPrimitivePassBatch {
                    key,
                    buffer,
                    count: instances.len() as u32,
                })
            })
            .collect()
    }

    pub fn for_each_resident_depth_batch<'a, F>(
        &'a self,
        chunk_refs: &[PrimitiveChunkRef],
        mut f: F,
    ) where
        F: FnMut(PrimitivePassBatchKey, &'a wgpu::Buffer, u32),
    {
        let mut visible_batches = Vec::new();
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for batch in &chunk.depth_batches {
                visible_batches.push(ResidentPrimitivePassBatchRef { batch });
            }
        }
        visible_batches.sort_by(compare_resident_primitive_pass_batch_refs);
        for batch_ref in visible_batches {
            let batch = batch_ref.batch;
            f(batch.key, &batch.buffer, batch.count);
        }
    }

    pub fn for_each_resident_shadow_batch<'a, F>(
        &'a self,
        chunk_refs: &[PrimitiveChunkRef],
        mut f: F,
    ) where
        F: FnMut(PrimitivePassBatchKey, &'a wgpu::Buffer, u32),
    {
        let mut visible_batches = Vec::new();
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for batch in &chunk.shadow_batches {
                visible_batches.push(ResidentPrimitivePassBatchRef { batch });
            }
        }
        visible_batches.sort_by(compare_resident_primitive_pass_batch_refs);
        for batch_ref in visible_batches {
            let batch = batch_ref.batch;
            f(batch.key, &batch.buffer, batch.count);
        }
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_instance_vb"),
            size: std::mem::size_of::<PrimitiveInstanceGpu>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    fn create_texture_bind_group(
        &self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: PrimitiveTextureKey,
        shadow_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let albedo_view = self.texture_view(textures, key.albedo);
        let normal_view = self.texture_view(textures, key.normal);
        let metallic_roughness_view = self.texture_view(textures, key.metallic_roughness);
        let emissive_view = self.texture_view(textures, key.emissive);
        let toon_ramp_view = self.texture_view(textures, key.toon_ramp);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_texture_bg"),
            layout: &self.main_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(
                        &self.material_samplers[key.sampler.sampler_index()],
                    ),
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
                    resource: wgpu::BindingResource::TextureView(&self.white_texture_view),
                },
            ],
        })
    }

    fn texture_view<'a>(
        &'a self,
        textures: &'a TextureManager,
        texture_id: u32,
    ) -> &'a wgpu::TextureView {
        textures
            .get(texture_id)
            .map(|texture| &texture.view)
            .unwrap_or(&self.white_texture_view)
    }

    fn ensure_texture_bind_group(
        &mut self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: PrimitiveTextureKey,
        shadow_view: &wgpu::TextureView,
    ) {
        if self.texture_bind_groups.contains_key(&key) {
            return;
        }
        let bind_group = self.create_texture_bind_group(device, textures, key, shadow_view);
        self.texture_bind_groups.insert(key, bind_group);
    }
}
