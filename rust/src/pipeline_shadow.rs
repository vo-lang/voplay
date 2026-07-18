use crate::animation;
use crate::model_loader::{MeshVertex, ModelManager, SkinnedMeshVertex};
use crate::pipeline3d::ModelDraw;
use crate::primitive_pipeline::{PrimitivePassInstanceGpu, PrimitivePipeline};
use crate::primitive_scene::{PrimitiveChunkRef, PrimitiveDraw};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

const MAX_JOINTS: usize = animation::MAX_JOINTS;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct LightVPUniform {
    pub light_vp: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShadowModelUniform {
    pub model: [[f32; 4]; 4],
    pub joint_count: [u32; 4],
    pub joints: [[[f32; 4]; 4]; MAX_JOINTS],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ShadowInstanceData {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
}

impl ShadowInstanceData {
    const ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }

    fn from_model(model: [[f32; 4]; 4]) -> Self {
        Self {
            model_0: model[0],
            model_1: model[1],
            model_2: model[2],
            model_3: model[3],
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct ShadowInstanceBatchKey {
    model_id: u32,
    mesh_index: usize,
}

struct ShadowInstanceBatch {
    key: ShadowInstanceBatchKey,
    instances: Vec<ShadowInstanceData>,
}

struct ShadowInstanceBatchDraw {
    key: ShadowInstanceBatchKey,
    start: u32,
    count: u32,
}

struct ShadowInstanceBatchData {
    draws: Vec<ShadowInstanceBatchDraw>,
    instances: Vec<ShadowInstanceData>,
}

pub struct PipelineShadow {
    pipeline_static: wgpu::RenderPipeline,
    pipeline_instanced: wgpu::RenderPipeline,
    pipeline_skinned: wgpu::RenderPipeline,
    light_vp_buffer: wgpu::Buffer,
    light_vp_bind_group: wgpu::BindGroup,
    cascade_light_vp_buffers: Vec<wgpu::Buffer>,
    cascade_light_vp_bind_groups: Vec<wgpu::BindGroup>,
    model_bgl: wgpu::BindGroupLayout,
    model_buffer: wgpu::Buffer,
    model_bind_group: wgpu::BindGroup,
    model_buffer_alignment: u32,
    model_buffer_slot_count: u32,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: u32,
    last_primitive_batch_count: u32,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    size: u32,
}

impl PipelineShadow {
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
        });

        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_shadow_light_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_shadow_model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let model_buffer_alignment = device.limits().min_uniform_buffer_offset_alignment;

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_shadow_layout"),
            bind_group_layouts: &[&light_bgl, &model_bgl],
            push_constant_ranges: &[],
        });
        let instanced_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("voplay_shadow_instanced_layout"),
                bind_group_layouts: &[&light_bgl],
                push_constant_ranges: &[],
            });

        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        };

        let depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        });

        let pipeline_static = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shadow_static"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                buffers: &[MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let pipeline_instanced = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shadow_instanced"),
            layout: Some(&instanced_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow_instanced"),
                buffers: &[MeshVertex::layout(), ShadowInstanceData::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let pipeline_skinned = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shadow_skinned"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow_skinned"),
                buffers: &[SkinnedMeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let light_vp_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_light_vp_ub"),
            size: std::mem::size_of::<LightVPUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_vp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_shadow_light_vp_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_vp_buffer.as_entire_binding(),
            }],
        });
        let mut cascade_light_vp_buffers = Vec::with_capacity(4);
        let mut cascade_light_vp_bind_groups = Vec::with_capacity(4);
        for index in 0..4 {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("voplay_shadow_cascade_light_vp_ub_{index}")),
                size: std::mem::size_of::<LightVPUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("voplay_shadow_cascade_light_vp_bg_{index}")),
                layout: &light_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            cascade_light_vp_buffers.push(buffer);
            cascade_light_vp_bind_groups.push(bind_group);
        }

        let model_buffer_slot_count: u32 = 256;
        let aligned_shadow_size = Self::align_up(
            std::mem::size_of::<ShadowModelUniform>() as u32,
            model_buffer_alignment,
        );
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_model_ub"),
            size: aligned_shadow_size as u64 * model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = Self::create_model_bind_group(
            device,
            &model_bgl,
            &model_buffer,
            std::mem::size_of::<ShadowModelUniform>() as u64,
        );
        let instance_buffer_capacity = 1024u32;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_instance_vb"),
            size: std::mem::size_of::<ShadowInstanceData>() as u64
                * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (depth_texture, depth_view) = Self::create_depth_texture(device, size.max(1));

        Self {
            pipeline_static,
            pipeline_instanced,
            pipeline_skinned,
            light_vp_buffer,
            light_vp_bind_group,
            cascade_light_vp_buffers,
            cascade_light_vp_bind_groups,
            model_bgl,
            model_buffer,
            model_bind_group,
            model_buffer_alignment,
            model_buffer_slot_count,
            instance_buffer,
            instance_buffer_capacity,
            last_primitive_batch_count: 0,
            depth_texture,
            depth_view,
            size: size.max(1),
        }
    }

    pub fn last_primitive_batch_count(&self) -> u32 {
        self.last_primitive_batch_count
    }

    fn create_depth_texture(
        device: &wgpu::Device,
        size: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_shadow_depth"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        (depth_texture, depth_view)
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: u32) {
        let size = size.max(1);
        if size == self.size {
            return;
        }
        let (depth_texture, depth_view) = Self::create_depth_texture(device, size);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        self.size = size;
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn shadow_texture_view(&self) -> &wgpu::TextureView {
        &self.depth_view
    }

    fn align_up(value: u32, alignment: u32) -> u32 {
        (value + alignment - 1) & !(alignment - 1)
    }

    fn create_model_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        binding_size: u64,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_shadow_model_bg"),
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

    fn ensure_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.model_buffer_slot_count {
            return;
        }
        let new_count = needed.next_power_of_two().max(256);
        let aligned = Self::align_up(
            std::mem::size_of::<ShadowModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.model_buffer,
            std::mem::size_of::<ShadowModelUniform>() as u64,
        );
        self.model_buffer_slot_count = new_count;
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_instance_vb"),
            size: std::mem::size_of::<ShadowInstanceData>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    fn build_primitive_batch_data(
        &self,
        draws: &[PrimitiveDraw],
        models: &ModelManager,
        base_instance: u32,
    ) -> ShadowInstanceBatchData {
        if draws.is_empty() {
            return ShadowInstanceBatchData {
                draws: Vec::new(),
                instances: Vec::new(),
            };
        }
        let mut batches: Vec<ShadowInstanceBatch> = Vec::new();
        let mut batch_index: HashMap<ShadowInstanceBatchKey, usize> = HashMap::new();

        for draw in draws {
            let Some(gpu_model) = models.get(draw.model_id) else {
                continue;
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                if mesh.skinned {
                    continue;
                }
                let key = ShadowInstanceBatchKey {
                    model_id: draw.model_id,
                    mesh_index,
                };
                let index = if let Some(index) = batch_index.get(&key) {
                    *index
                } else {
                    let index = batches.len();
                    batches.push(ShadowInstanceBatch {
                        key,
                        instances: Vec::new(),
                    });
                    batch_index.insert(key, index);
                    index
                };
                batches[index]
                    .instances
                    .push(ShadowInstanceData::from_model(draw.model_uniform.model));
            }
        }

        let instance_count: usize = batches.iter().map(|batch| batch.instances.len()).sum();
        if instance_count == 0 {
            return ShadowInstanceBatchData {
                draws: Vec::new(),
                instances: Vec::new(),
            };
        }
        let mut instance_data = Vec::with_capacity(instance_count);
        let mut batch_draws = Vec::with_capacity(batches.len());
        for batch in &batches {
            let start = base_instance + instance_data.len() as u32;
            instance_data.extend_from_slice(&batch.instances);
            batch_draws.push(ShadowInstanceBatchDraw {
                key: batch.key,
                start,
                count: batch.instances.len() as u32,
            });
        }
        ShadowInstanceBatchData {
            draws: batch_draws,
            instances: instance_data,
        }
    }

    fn build_primitive_batches(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[PrimitiveDraw],
        models: &ModelManager,
    ) -> Vec<ShadowInstanceBatchDraw> {
        let data = self.build_primitive_batch_data(draws, models, 0);
        if data.instances.is_empty() {
            return Vec::new();
        }
        self.ensure_instance_capacity(device, data.instances.len() as u32);
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&data.instances),
        );
        data.draws
    }

    // Clippy exception — owner: voplay/render; reason: argument order follows the stable shadow
    // pass contract across frame resources, light data, scene data, and model stores; expiry:
    // remove when the render graph owns a typed shadow-pass descriptor shared by every backend.
    #[allow(clippy::too_many_arguments)]
    pub fn render_shadow_pass(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        light_vp: &[[f32; 4]; 4],
        draws: &[ModelDraw],
        primitive_draws: &[PrimitiveDraw],
        primitive_chunk_refs: &[PrimitiveChunkRef],
        primitive_pipeline: &PrimitivePipeline,
        models: &ModelManager,
    ) {
        self.last_primitive_batch_count = 0;
        if draws.is_empty() && primitive_draws.is_empty() && primitive_chunk_refs.is_empty() {
            return;
        }

        let aligned_stride = Self::align_up(
            std::mem::size_of::<ShadowModelUniform>() as u32,
            self.model_buffer_alignment,
        );

        // Phase 1: count and upload all uniforms at aligned offsets
        let mut slot_count: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            slot_count += gpu_model.meshes.len() as u32;
        }
        self.ensure_capacity(device, slot_count);

        let primitive_batches =
            self.build_primitive_batches(device, queue, primitive_draws, models);
        self.last_primitive_batch_count = primitive_batches.len() as u32;

        let mut slot: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for mesh in &gpu_model.meshes {
                let mut uniform = ShadowModelUniform {
                    model: draw.model_uniform.model,
                    joint_count: [0, 0, 0, 0],
                    joints: [[[0.0; 4]; 4]; MAX_JOINTS],
                };
                if mesh.skinned {
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    assert!(
                        joint_palette.len() <= MAX_JOINTS,
                        "voplay: shadow joint palette exceeds MAX_JOINTS"
                    );
                    uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        uniform.joints[index] = *matrix;
                    }
                }
                let offset = slot as u64 * aligned_stride as u64;
                queue.write_buffer(&self.model_buffer, offset, bytemuck::bytes_of(&uniform));
                slot += 1;
            }
        }

        // Phase 2: issue draw calls with dynamic offsets
        let light_uniform = LightVPUniform {
            light_vp: *light_vp,
        };
        queue.write_buffer(&self.light_vp_buffer, 0, bytemuck::bytes_of(&light_uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_shadow_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_bind_group(0, &self.light_vp_bind_group, &[]);

        slot = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(model) => model,
                None => continue,
            };

            for mesh in &gpu_model.meshes {
                let dyn_offset = slot * aligned_stride;
                pass.set_bind_group(1, &self.model_bind_group, &[dyn_offset]);
                slot += 1;

                if mesh.skinned {
                    pass.set_pipeline(&self.pipeline_skinned);
                } else {
                    pass.set_pipeline(&self.pipeline_static);
                }

                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        if !primitive_batches.is_empty() {
            pass.set_pipeline(&self.pipeline_instanced);
            pass.set_bind_group(0, &self.light_vp_bind_group, &[]);
            let instance_stride = std::mem::size_of::<ShadowInstanceData>() as u64;
            for batch in &primitive_batches {
                let Some(gpu_model) = models.get(batch.key.model_id) else {
                    continue;
                };
                let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                    continue;
                };
                let start = batch.start as u64 * instance_stride;
                let end = start + batch.count as u64 * instance_stride;
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
            }
        }
        self.draw_resident_primitive_chunks(
            &mut pass,
            &self.light_vp_bind_group,
            primitive_pipeline,
            primitive_chunk_refs,
            models,
        );
    }

    // Clippy exception — owner: voplay/render; reason: adjacent cascade-indexed slices encode the
    // stable shadow-cascade pass contract; expiry: remove when a typed cascade-pass descriptor
    // owns the corresponding light, primitive, and resident-chunk slices.
    #[allow(clippy::too_many_arguments)]
    pub fn render_shadow_cascade_pass(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        light_vps: &[[[f32; 4]; 4]],
        draws: &[ModelDraw],
        primitive_draws: &[PrimitiveDraw],
        primitive_draws_by_cascade: &[Vec<PrimitiveDraw>],
        primitive_chunk_refs: &[PrimitiveChunkRef],
        primitive_chunk_refs_by_cascade: &[Vec<PrimitiveChunkRef>],
        primitive_pipeline: &PrimitivePipeline,
        models: &ModelManager,
    ) {
        self.last_primitive_batch_count = 0;
        let cascade_count = light_vps
            .len()
            .min(self.cascade_light_vp_buffers.len())
            .min(4);
        let has_cascade_primitives = primitive_draws_by_cascade
            .iter()
            .any(|draws| !draws.is_empty())
            || primitive_chunk_refs_by_cascade
                .iter()
                .any(|chunks| !chunks.is_empty());
        if cascade_count == 0
            || (draws.is_empty()
                && primitive_draws.is_empty()
                && primitive_chunk_refs.is_empty()
                && !has_cascade_primitives)
        {
            return;
        }

        let aligned_stride = Self::align_up(
            std::mem::size_of::<ShadowModelUniform>() as u32,
            self.model_buffer_alignment,
        );

        let mut slot_count: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            slot_count += gpu_model.meshes.len() as u32;
        }
        self.ensure_capacity(device, slot_count);

        let mut primitive_batches_by_cascade: Vec<Vec<ShadowInstanceBatchDraw>> =
            Vec::with_capacity(cascade_count);
        let mut all_primitive_instances: Vec<ShadowInstanceData> = Vec::new();
        for cascade_index in 0..cascade_count {
            let cascade_primitive_draws = primitive_draws_by_cascade
                .get(cascade_index)
                .map(Vec::as_slice)
                .unwrap_or(primitive_draws);
            let data = self.build_primitive_batch_data(
                cascade_primitive_draws,
                models,
                all_primitive_instances.len() as u32,
            );
            all_primitive_instances.extend_from_slice(&data.instances);
            primitive_batches_by_cascade.push(data.draws);
        }
        self.last_primitive_batch_count = primitive_batches_by_cascade
            .iter()
            .map(|batches| batches.len() as u32)
            .sum();
        if !all_primitive_instances.is_empty() {
            self.ensure_instance_capacity(device, all_primitive_instances.len() as u32);
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&all_primitive_instances),
            );
        }

        let mut slot: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for mesh in &gpu_model.meshes {
                let mut uniform = ShadowModelUniform {
                    model: draw.model_uniform.model,
                    joint_count: [0, 0, 0, 0],
                    joints: [[[0.0; 4]; 4]; MAX_JOINTS],
                };
                if mesh.skinned {
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    assert!(
                        joint_palette.len() <= MAX_JOINTS,
                        "voplay: shadow joint palette exceeds MAX_JOINTS"
                    );
                    uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        uniform.joints[index] = *matrix;
                    }
                }
                let offset = slot as u64 * aligned_stride as u64;
                queue.write_buffer(&self.model_buffer, offset, bytemuck::bytes_of(&uniform));
                slot += 1;
            }
        }

        for (index, light_vp) in light_vps.iter().take(cascade_count).enumerate() {
            let light_uniform = LightVPUniform {
                light_vp: *light_vp,
            };
            queue.write_buffer(
                &self.cascade_light_vp_buffers[index],
                0,
                bytemuck::bytes_of(&light_uniform),
            );
        }

        let tile_size = (self.size / 2).max(1);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_shadow_cascade_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        for cascade_index in 0..cascade_count {
            let tile_x = (cascade_index as u32 % 2) * tile_size;
            let tile_y = (cascade_index as u32 / 2) * tile_size;
            pass.set_viewport(
                tile_x as f32,
                tile_y as f32,
                tile_size as f32,
                tile_size as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(tile_x, tile_y, tile_size, tile_size);
            pass.set_bind_group(0, &self.cascade_light_vp_bind_groups[cascade_index], &[]);

            slot = 0;
            for draw in draws {
                let gpu_model = match models.get(draw.model_id) {
                    Some(model) => model,
                    None => continue,
                };

                for mesh in &gpu_model.meshes {
                    let dyn_offset = slot * aligned_stride;
                    pass.set_bind_group(1, &self.model_bind_group, &[dyn_offset]);
                    slot += 1;

                    if mesh.skinned {
                        pass.set_pipeline(&self.pipeline_skinned);
                    } else {
                        pass.set_pipeline(&self.pipeline_static);
                    }

                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }

            let primitive_batches = primitive_batches_by_cascade
                .get(cascade_index)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !primitive_batches.is_empty() {
                pass.set_pipeline(&self.pipeline_instanced);
                pass.set_bind_group(0, &self.cascade_light_vp_bind_groups[cascade_index], &[]);
                let instance_stride = std::mem::size_of::<ShadowInstanceData>() as u64;
                for batch in primitive_batches {
                    let Some(gpu_model) = models.get(batch.key.model_id) else {
                        continue;
                    };
                    let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                        continue;
                    };
                    let start = batch.start as u64 * instance_stride;
                    let end = start + batch.count as u64 * instance_stride;
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
                }
            }
            let cascade_primitive_chunk_refs = primitive_chunk_refs_by_cascade
                .get(cascade_index)
                .map(Vec::as_slice)
                .unwrap_or(primitive_chunk_refs);
            self.draw_resident_primitive_chunks(
                &mut pass,
                &self.cascade_light_vp_bind_groups[cascade_index],
                primitive_pipeline,
                cascade_primitive_chunk_refs,
                models,
            );
        }
    }

    fn draw_resident_primitive_chunks<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        light_bind_group: &'a wgpu::BindGroup,
        primitive_pipeline: &'a PrimitivePipeline,
        primitive_chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
    ) {
        if primitive_chunk_refs.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline_instanced);
        pass.set_bind_group(0, light_bind_group, &[]);
        let instance_stride = std::mem::size_of::<PrimitivePassInstanceGpu>() as u64;
        primitive_pipeline.for_each_resident_shadow_batch(
            primitive_chunk_refs,
            |key, buffer, count| {
                let Some(gpu_model) = models.get(key.model_id) else {
                    return;
                };
                let Some(mesh) = gpu_model.meshes.get(key.mesh_index) else {
                    return;
                };
                let end = count as u64 * instance_stride;
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, buffer.slice(0..end));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..count);
            },
        );
    }
}

// This test requests an OS-native wgpu adapter synchronously through pollster.
#[cfg(all(test, feature = "native", not(target_arch = "wasm32")))]
mod tests {
    use super::PipelineShadow;

    #[test]
    fn pipeline_shadow_creates_with_current_vertex_layouts() {
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
        let (device, _) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voplay_shadow_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let _pipeline = PipelineShadow::new(&device, 1024);
    }
}
