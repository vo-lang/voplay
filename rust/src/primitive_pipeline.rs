use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::material::{MaterialSamplerKey, MATERIAL_SAMPLER_KEYS};
use crate::model_loader::{MeshMaterial, MeshVertex, ModelId, ModelManager};
use crate::pipeline3d::{Camera3DUniform, LightUniform, MaterialOverride, ModelUniform};
use crate::primitive_scene::{PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate};
use crate::texture::TextureManager;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PrimitiveInstanceGpu {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
    normal_0: [f32; 4],
    normal_1: [f32; 4],
    normal_2: [f32; 4],
    base_color: [f32; 4],
    material_params: [f32; 4],
    emissive_color: [f32; 4],
    texture_flags: [f32; 4],
}

impl PrimitiveInstanceGpu {
    const ATTRIBS: [wgpu::VertexAttribute; 11] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Float32x4,
        11 => Float32x4,
        12 => Float32x4,
        13 => Float32x4,
        14 => Float32x4,
        15 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }

    fn from_uniform(uniform: &ModelUniform) -> Self {
        Self {
            model_0: uniform.model[0],
            model_1: uniform.model[1],
            model_2: uniform.model[2],
            model_3: uniform.model[3],
            normal_0: uniform.normal_matrix[0],
            normal_1: uniform.normal_matrix[1],
            normal_2: uniform.normal_matrix[2],
            base_color: uniform.base_color,
            material_params: uniform.material_params,
            emissive_color: uniform.emissive_color,
            texture_flags: uniform.texture_flags,
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveTextureKey {
    albedo: u32,
    normal: u32,
    metallic_roughness: u32,
    emissive: u32,
    toon_ramp: u32,
    sampler: MaterialSamplerKey,
}

impl PrimitiveTextureKey {
    fn has_albedo(self) -> bool {
        self.albedo != 0
    }

    fn texture_flags(self, normal_scale: f32) -> [f32; 4] {
        [
            if self.normal != 0 {
                normal_scale.max(0.0)
            } else {
                0.0
            },
            if self.metallic_roughness != 0 {
                1.0
            } else {
                0.0
            },
            if self.emissive != 0 { 1.0 } else { 0.0 },
            if self.toon_ramp != 0 { 1.0 } else { 0.0 },
        ]
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveBatchKey {
    model_id: ModelId,
    mesh_index: usize,
    textures: PrimitiveTextureKey,
}

struct PrimitiveBatch {
    key: PrimitiveBatchKey,
    instances: Vec<PrimitiveInstanceGpu>,
}

struct PrimitiveBatchDraw {
    key: PrimitiveBatchKey,
    start: u32,
    count: u32,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveObjectKey {
    scene_id: u32,
    layer_id: u32,
    object_id: u32,
}

#[derive(Clone, Copy)]
struct ResidentPrimitiveInstance {
    object_id: u32,
    draw: PrimitiveDraw,
}

struct ResidentPrimitiveChunk {
    instances: Vec<ResidentPrimitiveInstance>,
    batches: Vec<ResidentPrimitiveBatch>,
}

struct ResidentPrimitiveBatch {
    key: PrimitiveBatchKey,
    buffer: wgpu::Buffer,
    count: u32,
}

pub struct PrimitivePipeline {
    pipeline_textured: wgpu::RenderPipeline,
    pipeline_untextured: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    model_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: u32,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    main_texture_bind_group_layout: wgpu::BindGroupLayout,
    material_samplers: Vec<wgpu::Sampler>,
    white_texture_view: wgpu::TextureView,
    resident_chunks: HashMap<PrimitiveChunkRef, ResidentPrimitiveChunk>,
    object_chunks: HashMap<PrimitiveObjectKey, PrimitiveChunkRef>,
}

impl PrimitivePipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_primitive_mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
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
                visibility: wgpu::ShaderStages::FRAGMENT,
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
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let vertex = wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_instanced"),
            buffers: &[MeshVertex::layout(), PrimitiveInstanceGpu::layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        };
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        };
        let pipeline_textured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_primitive_instanced_textured"),
            layout: Some(&pipeline_layout),
            vertex: vertex.clone(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_instanced"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let pipeline_untextured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_primitive_instanced_untextured"),
            layout: Some(&pipeline_layout),
            vertex,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_instanced_no_tex"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

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
            pipeline_textured,
            pipeline_untextured,
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
        }
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

    pub fn replace_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: &[PrimitiveObjectUpdate],
        models: &ModelManager,
        textures: &TextureManager,
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
                        textures,
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
        let batches = self.build_resident_batches(device, queue, &instances, models, textures);
        self.resident_chunks
            .insert(chunk_ref, ResidentPrimitiveChunk { instances, batches });
    }

    pub fn upsert_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        update: PrimitiveObjectUpdate,
        models: &ModelManager,
        textures: &TextureManager,
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
            return;
        }
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            self.object_chunks.remove(&object_key);
            return;
        };
        if let Some(instance) = chunk
            .instances
            .iter_mut()
            .find(|instance| instance.object_id == update.object_id)
        {
            instance.draw = PrimitiveDraw::from_update(update);
        } else {
            chunk.instances.push(ResidentPrimitiveInstance {
                object_id: update.object_id,
                draw: PrimitiveDraw::from_update(update),
            });
        }
        self.rebuild_resident_chunk(device, queue, chunk_ref, models, textures);
    }

    pub fn destroy_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
        models: &ModelManager,
        textures: &TextureManager,
    ) {
        let object_key = PrimitiveObjectKey {
            scene_id,
            layer_id,
            object_id,
        };
        let Some(chunk_ref) = self.object_chunks.get(&object_key).copied() else {
            return;
        };
        self.remove_object_from_resident_chunk(
            device, queue, object_key, chunk_ref, models, textures,
        );
    }

    pub fn clear_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.resident_chunks.retain(|chunk_ref, _| {
            !(chunk_ref.scene_id == scene_id && chunk_ref.layer_id == layer_id)
        });
        self.object_chunks.retain(|object_key, _| {
            !(object_key.scene_id == scene_id && object_key.layer_id == layer_id)
        });
    }

    pub fn clear_scene(&mut self, scene_id: u32) {
        self.resident_chunks
            .retain(|chunk_ref, _| chunk_ref.scene_id != scene_id);
        self.object_chunks
            .retain(|object_key, _| object_key.scene_id != scene_id);
    }

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
    ) {
        if draws.is_empty() && chunk_refs.is_empty() {
            return;
        }
        let mut texture_bind_groups: HashMap<PrimitiveTextureKey, wgpu::BindGroup> = HashMap::new();
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.model_bind_group, &[0]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);
        if !draws.is_empty() {
            let mut batches: Vec<PrimitiveBatch> = Vec::new();
            let mut batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
            let mut instance_count = 0u32;
            for draw in draws {
                let Some(gpu_model) = models.get(draw.model_id) else {
                    continue;
                };
                for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                    if mesh.skinned || mesh.material.control_texture_id.is_some() {
                        continue;
                    }
                    let texture_key = resolve_texture_key(&draw.material, &mesh.material, textures);
                    let key = PrimitiveBatchKey {
                        model_id: draw.model_id,
                        mesh_index,
                        textures: texture_key,
                    };
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
                    let mut uniform = draw.model_uniform;
                    uniform.base_color =
                        combined_base_color(&draw.material, &mesh.material.base_color);
                    let (material_params, emissive_color, texture_flags) =
                        mesh_uniform_values(&draw.material, &mesh.material, texture_key);
                    uniform.material_params = material_params;
                    uniform.emissive_color = emissive_color;
                    uniform.texture_flags = texture_flags;
                    batches[index]
                        .instances
                        .push(PrimitiveInstanceGpu::from_uniform(&uniform));
                    instance_count += 1;
                }
            }
            if instance_count > 0 {
                self.ensure_instance_capacity(device, instance_count);
                let mut instance_data = Vec::with_capacity(instance_count as usize);
                let mut batch_draws = Vec::with_capacity(batches.len());
                for batch in &batches {
                    let start = instance_data.len() as u32;
                    instance_data.extend_from_slice(&batch.instances);
                    batch_draws.push(PrimitiveBatchDraw {
                        key: batch.key,
                        start,
                        count: batch.instances.len() as u32,
                    });
                }
                queue.write_buffer(
                    &self.instance_buffer,
                    0,
                    bytemuck::cast_slice(&instance_data),
                );

                let instance_stride = std::mem::size_of::<PrimitiveInstanceGpu>() as u64;

                for batch in &batch_draws {
                    let Some(gpu_model) = models.get(batch.key.model_id) else {
                        continue;
                    };
                    let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                        continue;
                    };
                    let texture_key = batch.key.textures;
                    let texture_bind_group =
                        texture_bind_groups.entry(texture_key).or_insert_with(|| {
                            self.create_texture_bind_group(
                                device,
                                textures,
                                texture_key,
                                shadow_view,
                            )
                        });
                    if texture_key.has_albedo() {
                        pass.set_pipeline(&self.pipeline_textured);
                    } else {
                        pass.set_pipeline(&self.pipeline_untextured);
                    }
                    pass.set_bind_group(3, &*texture_bind_group, &[]);
                    let start = batch.start as u64 * instance_stride;
                    let end = start + batch.count as u64 * instance_stride;
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
                }
            }
        }
        self.draw_resident_chunks(
            device,
            pass,
            chunk_refs,
            models,
            textures,
            shadow_view,
            &mut texture_bind_groups,
        );
    }

    fn draw_resident_chunks<'a>(
        &'a self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'a>,
        chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        texture_bind_groups: &mut HashMap<PrimitiveTextureKey, wgpu::BindGroup>,
    ) {
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for batch in &chunk.batches {
                let Some(gpu_model) = models.get(batch.key.model_id) else {
                    continue;
                };
                let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                    continue;
                };
                let texture_key = batch.key.textures;
                let texture_bind_group =
                    texture_bind_groups.entry(texture_key).or_insert_with(|| {
                        self.create_texture_bind_group(device, textures, texture_key, shadow_view)
                    });
                if texture_key.has_albedo() {
                    pass.set_pipeline(&self.pipeline_textured);
                } else {
                    pass.set_pipeline(&self.pipeline_untextured);
                }
                pass.set_bind_group(3, &*texture_bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, batch.buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
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
    ) {
        let Some(gpu_model) = models.get(draw.model_id) else {
            return;
        };
        for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
            if mesh.skinned || mesh.material.control_texture_id.is_some() {
                continue;
            }
            let texture_key = resolve_texture_key(&draw.material, &mesh.material, textures);
            let key = PrimitiveBatchKey {
                model_id: draw.model_id,
                mesh_index,
                textures: texture_key,
            };
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
            let mut uniform = draw.model_uniform;
            uniform.base_color = combined_base_color(&draw.material, &mesh.material.base_color);
            let (material_params, emissive_color, texture_flags) =
                mesh_uniform_values(&draw.material, &mesh.material, texture_key);
            uniform.material_params = material_params;
            uniform.emissive_color = emissive_color;
            uniform.texture_flags = texture_flags;
            batches[index]
                .instances
                .push(PrimitiveInstanceGpu::from_uniform(&uniform));
        }
    }

    fn remove_object_from_resident_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        object_key: PrimitiveObjectKey,
        chunk_ref: PrimitiveChunkRef,
        models: &ModelManager,
        textures: &TextureManager,
    ) {
        self.object_chunks.remove(&object_key);
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            return;
        };
        chunk
            .instances
            .retain(|instance| instance.object_id != object_key.object_id);
        self.rebuild_resident_chunk(device, queue, chunk_ref, models, textures);
    }

    fn rebuild_resident_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunk_ref: PrimitiveChunkRef,
        models: &ModelManager,
        textures: &TextureManager,
    ) {
        let Some(instances) = self.resident_chunks.get(&chunk_ref).map(|chunk| {
            chunk
                .instances
                .iter()
                .map(|instance| ResidentPrimitiveInstance {
                    object_id: instance.object_id,
                    draw: instance.draw,
                })
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let batches = self.build_resident_batches(device, queue, &instances, models, textures);
        if instances.is_empty() {
            self.resident_chunks.remove(&chunk_ref);
        } else if let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) {
            chunk.batches = batches;
        }
    }

    fn build_resident_batches(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[ResidentPrimitiveInstance],
        models: &ModelManager,
        textures: &TextureManager,
    ) -> Vec<ResidentPrimitiveBatch> {
        let mut batches: Vec<PrimitiveBatch> = Vec::new();
        let mut batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
        for instance in instances {
            self.push_draw_batches(
                &instance.draw,
                models,
                textures,
                &mut batches,
                &mut batch_index,
            );
        }
        batches
            .into_iter()
            .filter_map(|batch| {
                if batch.instances.is_empty() {
                    return None;
                }
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("voplay_primitive_chunk_instance_vb"),
                    size: std::mem::size_of::<PrimitiveInstanceGpu>() as u64
                        * batch.instances.len() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&batch.instances));
                Some(ResidentPrimitiveBatch {
                    key: batch.key,
                    buffer,
                    count: batch.instances.len() as u32,
                })
            })
            .collect()
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
}

fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}

fn create_material_sampler(device: &wgpu::Device, key: MaterialSamplerKey) -> wgpu::Sampler {
    let address_mode = match key.wrap_mode {
        crate::material::MATERIAL_WRAP_CLAMP => wgpu::AddressMode::ClampToEdge,
        crate::material::MATERIAL_WRAP_MIRROR => wgpu::AddressMode::MirrorRepeat,
        _ => wgpu::AddressMode::Repeat,
    };
    let filter = match key.filter_mode {
        crate::material::MATERIAL_FILTER_NEAREST => wgpu::FilterMode::Nearest,
        _ => wgpu::FilterMode::Linear,
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("voplay_primitive_material_sampler"),
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        ..Default::default()
    })
}

fn valid_texture_id(textures: &TextureManager, texture_id: Option<u32>) -> u32 {
    texture_id
        .filter(|id| *id != 0 && textures.get(*id).is_some())
        .unwrap_or(0)
}

fn resolve_texture_key(
    material: &MaterialOverride,
    mesh_material: &MeshMaterial,
    textures: &TextureManager,
) -> PrimitiveTextureKey {
    let albedo =
        if material.albedo_texture_id != 0 && textures.get(material.albedo_texture_id).is_some() {
            material.albedo_texture_id
        } else {
            valid_texture_id(textures, mesh_material.texture_id)
        };
    let normal =
        if material.normal_texture_id != 0 && textures.get(material.normal_texture_id).is_some() {
            material.normal_texture_id
        } else {
            valid_texture_id(textures, mesh_material.normal_texture_id)
        };
    let metallic_roughness = if material.metallic_roughness_texture_id != 0
        && textures
            .get(material.metallic_roughness_texture_id)
            .is_some()
    {
        material.metallic_roughness_texture_id
    } else {
        valid_texture_id(textures, mesh_material.metallic_roughness_texture_id)
    };
    let emissive = if material.emissive_texture_id != 0
        && textures.get(material.emissive_texture_id).is_some()
    {
        material.emissive_texture_id
    } else {
        valid_texture_id(textures, mesh_material.emissive_texture_id)
    };
    let toon_ramp = if material.toon_ramp_texture_id != 0
        && textures.get(material.toon_ramp_texture_id).is_some()
    {
        material.toon_ramp_texture_id
    } else {
        valid_texture_id(textures, mesh_material.toon_ramp_texture_id)
    };
    PrimitiveTextureKey {
        albedo,
        normal,
        metallic_roughness,
        emissive,
        toon_ramp,
        sampler: MaterialSamplerKey::resolve(
            material.wrap_mode,
            material.filter_mode,
            mesh_material.sampler,
        ),
    }
}

fn combined_base_color(material: &MaterialOverride, mesh_color: &[f32; 4]) -> [f32; 4] {
    let color = material.base_color_multiplier();
    [
        mesh_color[0] * color[0],
        mesh_color[1] * color[1],
        mesh_color[2] * color[2],
        mesh_color[3] * color[3],
    ]
}

fn mesh_uniform_values(
    material: &MaterialOverride,
    mesh_material: &MeshMaterial,
    key: PrimitiveTextureKey,
) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let override_emissive_texture_active =
        material.emissive_texture_id != 0 && key.emissive == material.emissive_texture_id;
    (
        [
            material.uv_scale_or(mesh_material.uv_scales[0]),
            roughness_value(material, mesh_material.roughness),
            metallic_value(material, mesh_material.metallic),
            if material.shading_mode == 0 {
                0.0
            } else {
                material.shading_mode as f32
            },
        ],
        emissive_color_value(
            material,
            mesh_material.emissive_factor,
            override_emissive_texture_active,
        ),
        key.texture_flags(normal_scale_value(material, mesh_material.normal_scale)),
    )
}

fn roughness_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.roughness > 0.0 {
        material.roughness.clamp(0.04, 1.0)
    } else {
        fallback.clamp(0.04, 1.0)
    }
}

fn metallic_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.metallic != 0.0 {
        material.metallic.clamp(0.0, 1.0)
    } else {
        fallback.clamp(0.0, 1.0)
    }
}

fn normal_scale_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.normal_scale > 0.0 {
        material.normal_scale
    } else if material.normal_texture_id != 0 {
        1.0
    } else {
        fallback.max(0.0)
    }
}

fn emissive_color_value(
    material: &MaterialOverride,
    source_factor: [f32; 3],
    override_texture_active: bool,
) -> [f32; 4] {
    let e = material.emissive_color;
    if e[0] == 0.0 && e[1] == 0.0 && e[2] == 0.0 && e[3] == 0.0 {
        if source_factor != [0.0, 0.0, 0.0] {
            [source_factor[0], source_factor[1], source_factor[2], 1.0]
        } else if override_texture_active {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::PrimitivePipeline;
    use crate::math3d::{Quat, Vec3};
    use crate::model_loader::ModelManager;
    use crate::pipeline3d::MaterialOverride;
    use crate::primitive_scene::PrimitiveObjectUpdate;
    use crate::texture::TextureManager;

    #[test]
    fn primitive_pipeline_creates_with_current_shader_layouts() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voplay_primitive_pipeline_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let _pipeline =
            PrimitivePipeline::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb);
    }

    #[test]
    fn primitive_pipeline_tracks_resident_chunks() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voplay_primitive_resident_chunk_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");
        let mut pipeline =
            PrimitivePipeline::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb);
        let models = ModelManager::new();
        let textures = TextureManager::new(&device);
        let update = PrimitiveObjectUpdate {
            scene_id: 1,
            layer_id: 2,
            object_id: 3,
            model_id: 4,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        };
        pipeline.replace_chunk(&device, &queue, 1, 2, 5, &[update], &models, &textures);
        assert_eq!(pipeline.resident_chunks.len(), 1);
        assert_eq!(pipeline.object_chunks.len(), 1);

        pipeline.destroy_instance(&device, &queue, 1, 2, 3, &models, &textures);
        assert!(pipeline.resident_chunks.is_empty());
        assert!(pipeline.object_chunks.is_empty());
    }
}
