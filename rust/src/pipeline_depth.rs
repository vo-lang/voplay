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
struct DepthVPUniform {
    view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct DepthModelUniform {
    model: [[f32; 4]; 4],
    joint_count: [u32; 4],
    joints: [[[f32; 4]; 4]; MAX_JOINTS],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct DepthInstanceData {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
}

impl DepthInstanceData {
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
struct DepthInstanceBatchKey {
    model_id: u32,
    mesh_index: usize,
}

struct DepthInstanceBatch {
    key: DepthInstanceBatchKey,
    instances: Vec<DepthInstanceData>,
}

struct DepthInstanceBatchDraw {
    key: DepthInstanceBatchKey,
    start: u32,
    count: u32,
}

pub struct PipelineDepth {
    pipeline_static: wgpu::RenderPipeline,
    pipeline_instanced: wgpu::RenderPipeline,
    pipeline_skinned: wgpu::RenderPipeline,
    view_proj_buffer: wgpu::Buffer,
    view_proj_bind_group: wgpu::BindGroup,
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
    width: u32,
    height: u32,
}

impl PipelineDepth {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_camera_depth"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
        });

        let view_proj_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_camera_depth_vp_bgl"),
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
            label: Some("voplay_camera_depth_model_bgl"),
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
            label: Some("voplay_camera_depth_layout"),
            bind_group_layouts: &[&view_proj_bgl, &model_bgl],
            push_constant_ranges: &[],
        });
        let instanced_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("voplay_camera_depth_instanced_layout"),
                bind_group_layouts: &[&view_proj_bgl],
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
            bias: wgpu::DepthBiasState::default(),
        });

        let pipeline_static = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_camera_depth_static"),
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
            label: Some("voplay_camera_depth_instanced"),
            layout: Some(&instanced_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow_instanced"),
                buffers: &[MeshVertex::layout(), DepthInstanceData::layout()],
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
            label: Some("voplay_camera_depth_skinned"),
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

        let view_proj_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera_depth_vp_ub"),
            size: std::mem::size_of::<DepthVPUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_proj_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_camera_depth_vp_bg"),
            layout: &view_proj_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_proj_buffer.as_entire_binding(),
            }],
        });

        let model_buffer_slot_count: u32 = 256;
        let aligned_depth_size = Self::align_up(
            std::mem::size_of::<DepthModelUniform>() as u32,
            model_buffer_alignment,
        );
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera_depth_model_ub"),
            size: aligned_depth_size as u64 * model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = Self::create_model_bind_group(
            device,
            &model_bgl,
            &model_buffer,
            std::mem::size_of::<DepthModelUniform>() as u64,
        );

        let instance_buffer_capacity = 1024u32;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera_depth_instance_vb"),
            size: std::mem::size_of::<DepthInstanceData>() as u64 * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (depth_texture, depth_view) =
            Self::create_depth_texture(device, width.max(1), height.max(1));

        Self {
            pipeline_static,
            pipeline_instanced,
            pipeline_skinned,
            view_proj_buffer,
            view_proj_bind_group,
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
            width: width.max(1),
            height: height.max(1),
        }
    }

    pub fn last_primitive_batch_count(&self) -> u32 {
        self.last_primitive_batch_count
    }

    fn create_depth_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_camera_depth_texture"),
            size: wgpu::Extent3d {
                width,
                height,
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

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return;
        }
        let (depth_texture, depth_view) = Self::create_depth_texture(device, width, height);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        self.width = width;
        self.height = height;
    }

    pub fn depth_texture_view(&self) -> &wgpu::TextureView {
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
            label: Some("voplay_camera_depth_model_bg"),
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
            std::mem::size_of::<DepthModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera_depth_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.model_buffer,
            std::mem::size_of::<DepthModelUniform>() as u64,
        );
        self.model_buffer_slot_count = new_count;
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera_depth_instance_vb"),
            size: std::mem::size_of::<DepthInstanceData>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    fn build_primitive_batches(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[PrimitiveDraw],
        models: &ModelManager,
    ) -> Vec<DepthInstanceBatchDraw> {
        if draws.is_empty() {
            return Vec::new();
        }
        let mut batches: Vec<DepthInstanceBatch> = Vec::new();
        let mut batch_index: HashMap<DepthInstanceBatchKey, usize> = HashMap::new();
        let mut instance_count = 0u32;

        for draw in draws {
            let Some(gpu_model) = models.get(draw.model_id) else {
                continue;
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                if mesh.skinned {
                    continue;
                }
                let key = DepthInstanceBatchKey {
                    model_id: draw.model_id,
                    mesh_index,
                };
                let index = if let Some(index) = batch_index.get(&key) {
                    *index
                } else {
                    let index = batches.len();
                    batches.push(DepthInstanceBatch {
                        key,
                        instances: Vec::new(),
                    });
                    batch_index.insert(key, index);
                    index
                };
                batches[index]
                    .instances
                    .push(DepthInstanceData::from_model(draw.model_uniform.model));
                instance_count += 1;
            }
        }

        if instance_count == 0 {
            return Vec::new();
        }
        self.ensure_instance_capacity(device, instance_count);

        let mut instance_data = Vec::with_capacity(instance_count as usize);
        let mut batch_draws = Vec::with_capacity(batches.len());
        for batch in &batches {
            let start = instance_data.len() as u32;
            instance_data.extend_from_slice(&batch.instances);
            batch_draws.push(DepthInstanceBatchDraw {
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
        batch_draws
    }

    pub fn render_depth_pass(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        view_proj: &[[f32; 4]; 4],
        draws: &[ModelDraw],
        primitive_draws: &[PrimitiveDraw],
        primitive_chunk_refs: &[PrimitiveChunkRef],
        primitive_pipeline: &PrimitivePipeline,
        models: &ModelManager,
    ) {
        self.last_primitive_batch_count = 0;
        if draws.is_empty() && primitive_draws.is_empty() && primitive_chunk_refs.is_empty() {
            let clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay_camera_depth_clear"),
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
            drop(clear_pass);
            return;
        }

        let aligned_stride = Self::align_up(
            std::mem::size_of::<DepthModelUniform>() as u32,
            self.model_buffer_alignment,
        );

        let mut slot_count: u32 = 0;
        for draw in draws {
            let Some(gpu_model) = models.get(draw.model_id) else {
                continue;
            };
            slot_count += gpu_model.meshes.len() as u32;
        }
        self.ensure_capacity(device, slot_count);

        let primitive_batches =
            self.build_primitive_batches(device, queue, primitive_draws, models);
        self.last_primitive_batch_count = primitive_batches.len() as u32;

        let mut slot: u32 = 0;
        for draw in draws {
            let Some(gpu_model) = models.get(draw.model_id) else {
                continue;
            };
            for mesh in &gpu_model.meshes {
                let mut uniform = DepthModelUniform {
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
                        "voplay: depth joint palette exceeds MAX_JOINTS"
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

        let vp_uniform = DepthVPUniform {
            view_proj: *view_proj,
        };
        queue.write_buffer(&self.view_proj_buffer, 0, bytemuck::bytes_of(&vp_uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_camera_depth_prepass"),
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

        pass.set_bind_group(0, &self.view_proj_bind_group, &[]);

        slot = 0;
        for draw in draws {
            let Some(gpu_model) = models.get(draw.model_id) else {
                continue;
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
            pass.set_bind_group(0, &self.view_proj_bind_group, &[]);
            let instance_stride = std::mem::size_of::<DepthInstanceData>() as u64;
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
            primitive_pipeline,
            primitive_chunk_refs,
            models,
        );
    }

    fn draw_resident_primitive_chunks<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        primitive_pipeline: &'a PrimitivePipeline,
        primitive_chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
    ) {
        if primitive_chunk_refs.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline_instanced);
        pass.set_bind_group(0, &self.view_proj_bind_group, &[]);
        let instance_stride = std::mem::size_of::<PrimitivePassInstanceGpu>() as u64;
        primitive_pipeline.for_each_resident_depth_batch(
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

#[cfg(test)]
mod tests {
    use super::PipelineDepth;

    #[test]
    fn pipeline_depth_creates_with_current_vertex_layouts() {
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
                label: Some("voplay_depth_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let pipeline = PipelineDepth::new(&device, 320, 180);
        let _view = pipeline.depth_texture_view();
    }
}
