use std::{collections::BTreeMap, num::NonZeroU64};

use voplay_runtime::render_feature::{AttachmentPoint, RealizedFeature, ShaderBindingKind};
use wgpu::util::DeviceExt;

use crate::WgpuSceneRendererError;

struct DecodedBinding {
    group: u32,
    binding: u32,
    kind: ShaderBindingKind,
    min_size: u64,
}

struct DecodedDataFeature<'a> {
    wgsl: &'a str,
    material_defaults: &'a [u8],
    sample_count: u32,
    groups: [u32; 4],
    bindings: Vec<DecodedBinding>,
}

enum OwnedBindingResource {
    Buffer(wgpu::Buffer),
    Texture {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
    },
    Sampler(wgpu::Sampler),
}

struct DataFeaturePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_groups: Vec<(u32, wgpu::BindGroup)>,
    _layouts: Vec<wgpu::BindGroupLayout>,
    _resources: Vec<OwnedBindingResource>,
    bindings: Vec<DecodedBinding>,
    groups: [u32; 4],
}

pub(crate) struct WgpuDataFeatureContext {
    pub world_revision: u64,
    pub view_revision: u64,
    pub width: u32,
    pub height: u32,
    pub sample_count: u32,
    pub camera_position_milli: [i64; 3],
    pub view_projection: [[f32; 4]; 4],
}

pub struct WgpuDataFeatureExecutor {
    pipelines: BTreeMap<u64, DataFeaturePipeline>,
    max_pipelines: usize,
}

impl WgpuDataFeatureExecutor {
    pub fn shutdown(&mut self) -> usize {
        let released = self.pipelines.len();
        self.pipelines.clear();
        released
    }

    pub fn new(max_pipelines: usize) -> Result<Self, WgpuSceneRendererError> {
        if max_pipelines == 0 {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        Ok(Self {
            pipelines: BTreeMap::new(),
            max_pipelines,
        })
    }

    pub fn encode_attachment(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        features: &[RealizedFeature],
        attachment: AttachmentPoint,
        context: &WgpuDataFeatureContext,
    ) -> Result<(), WgpuSceneRendererError> {
        let selected = features
            .iter()
            .filter(|feature| feature.attachment == attachment)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(());
        }
        for feature in &selected {
            self.ensure_pipeline(device, color_format, sample_count, feature)?;
            let pipeline = self
                .pipelines
                .get(&feature.pipeline_key)
                .ok_or(WgpuSceneRendererError::InvalidConfig)?;
            update_dynamic_bindings(queue, pipeline, context, attachment)?;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay-data-render-feature"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for feature in selected {
            let pipeline = self
                .pipelines
                .get(&feature.pipeline_key)
                .ok_or(WgpuSceneRendererError::InvalidConfig)?;
            pass.set_pipeline(&pipeline.pipeline);
            for (group, bind_group) in &pipeline.bind_groups {
                pass.set_bind_group(*group, bind_group, &[]);
            }
            pass.draw(0..3, 0..1);
        }
        Ok(())
    }

    fn ensure_pipeline(
        &mut self,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        feature: &RealizedFeature,
    ) -> Result<(), WgpuSceneRendererError> {
        if self.pipelines.contains_key(&feature.pipeline_key) {
            return Ok(());
        }
        if self.pipelines.len() == self.max_pipelines {
            return Err(WgpuSceneRendererError::Capacity);
        }
        let decoded = decode_data_feature(&feature.encoded_node)?;
        if decoded.sample_count != sample_count || !matches!(decoded.sample_count, 1 | 2 | 4 | 8) {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        let max_group = decoded
            .groups
            .iter()
            .copied()
            .chain(decoded.bindings.iter().map(|binding| binding.group))
            .max()
            .unwrap_or(0);
        if max_group > 15 {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        let mut layouts = Vec::with_capacity(max_group as usize + 1);
        for group in 0..=max_group {
            let entries = decoded
                .bindings
                .iter()
                .filter(|binding| binding.group == group)
                .map(binding_layout_entry)
                .collect::<Vec<_>>();
            layouts.push(
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("voplay-data-feature-layout"),
                    entries: &entries,
                }),
            );
        }
        let mut resources = Vec::with_capacity(decoded.bindings.len());
        for binding in &decoded.bindings {
            resources.push(create_resource(
                device,
                binding,
                if binding.group == decoded.groups[2] {
                    decoded.material_defaults
                } else {
                    &[]
                },
            )?);
        }
        let mut bind_groups = Vec::new();
        for group in 0..=max_group {
            let indexes = decoded
                .bindings
                .iter()
                .enumerate()
                .filter_map(|(index, binding)| (binding.group == group).then_some(index))
                .collect::<Vec<_>>();
            if indexes.is_empty() {
                continue;
            }
            let entries = indexes
                .iter()
                .map(|index| wgpu::BindGroupEntry {
                    binding: decoded.bindings[*index].binding,
                    resource: binding_resource(&resources[*index]),
                })
                .collect::<Vec<_>>();
            bind_groups.push((
                group,
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("voplay-data-feature-bind-group"),
                    layout: &layouts[group as usize],
                    entries: &entries,
                }),
            ));
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay-data-render-feature-wgsl"),
            source: wgpu::ShaderSource::Wgsl(decoded.wgsl.into()),
        });
        let layout_refs = layouts.iter().collect::<Vec<_>>();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay-data-feature-pipeline-layout"),
            bind_group_layouts: &layout_refs,
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay-data-render-feature-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: None,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: None,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: decoded.sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        self.pipelines.insert(
            feature.pipeline_key,
            DataFeaturePipeline {
                pipeline,
                bind_groups,
                _layouts: layouts,
                _resources: resources,
                bindings: decoded.bindings,
                groups: decoded.groups,
            },
        );
        Ok(())
    }
}

fn update_dynamic_bindings(
    queue: &wgpu::Queue,
    pipeline: &DataFeaturePipeline,
    context: &WgpuDataFeatureContext,
    attachment: AttachmentPoint,
) -> Result<(), WgpuSceneRendererError> {
    let frame = encode_frame_context(context, attachment);
    let view = encode_view_context(context);
    for (binding, resource) in pipeline.bindings.iter().zip(&pipeline._resources) {
        let bytes = if binding.group == pipeline.groups[0] {
            Some(frame.as_slice())
        } else if binding.group == pipeline.groups[1] {
            Some(view.as_slice())
        } else if binding.group == pipeline.groups[3] {
            Some(&[][..])
        } else {
            None
        };
        let (Some(bytes), OwnedBindingResource::Buffer(buffer)) = (bytes, resource) else {
            continue;
        };
        let size =
            usize::try_from(binding.min_size).map_err(|_| WgpuSceneRendererError::Capacity)?;
        let mut upload = vec![0; size.max(16).div_ceil(16) * 16];
        let copied = bytes.len().min(upload.len());
        upload[..copied].copy_from_slice(&bytes[..copied]);
        queue.write_buffer(buffer, 0, &upload);
    }
    Ok(())
}

fn encode_frame_context(context: &WgpuDataFeatureContext, attachment: AttachmentPoint) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(&context.world_revision.to_le_bytes());
    bytes.extend_from_slice(&context.view_revision.to_le_bytes());
    bytes.extend_from_slice(&context.width.to_le_bytes());
    bytes.extend_from_slice(&context.height.to_le_bytes());
    bytes.extend_from_slice(&context.sample_count.to_le_bytes());
    bytes.extend_from_slice(&(attachment as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

fn encode_view_context(context: &WgpuDataFeatureContext) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(96);
    for row in context.view_projection {
        for value in row {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in context.camera_position_milli {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&context.width.to_le_bytes());
    bytes.extend_from_slice(&context.height.to_le_bytes());
    bytes
}

fn binding_layout_entry(binding: &DecodedBinding) -> wgpu::BindGroupLayoutEntry {
    let ty = match binding.kind {
        ShaderBindingKind::Uniform => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(binding.min_size),
        },
        ShaderBindingKind::StorageRead => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(binding.min_size),
        },
        ShaderBindingKind::StorageReadWrite => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(binding.min_size),
        },
        ShaderBindingKind::Texture => wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        ShaderBindingKind::Sampler => {
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
        }
    };
    wgpu::BindGroupLayoutEntry {
        binding: binding.binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty,
        count: None,
    }
}

fn create_resource(
    device: &wgpu::Device,
    binding: &DecodedBinding,
    defaults: &[u8],
) -> Result<OwnedBindingResource, WgpuSceneRendererError> {
    Ok(match binding.kind {
        ShaderBindingKind::Uniform
        | ShaderBindingKind::StorageRead
        | ShaderBindingKind::StorageReadWrite => {
            let size =
                usize::try_from(binding.min_size).map_err(|_| WgpuSceneRendererError::Capacity)?;
            let aligned = size.max(defaults.len()).max(16).div_ceil(16) * 16;
            let mut bytes = vec![0; aligned];
            let copied = defaults.len().min(bytes.len());
            bytes[..copied].copy_from_slice(&defaults[..copied]);
            OwnedBindingResource::Buffer(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("voplay-data-feature-buffer"),
                    contents: &bytes,
                    usage: match binding.kind {
                        ShaderBindingKind::Uniform => wgpu::BufferUsages::UNIFORM,
                        _ => wgpu::BufferUsages::STORAGE,
                    },
                },
            ))
        }
        ShaderBindingKind::Texture => {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voplay-data-feature-texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            OwnedBindingResource::Texture {
                _texture: texture,
                view,
            }
        }
        ShaderBindingKind::Sampler => {
            OwnedBindingResource::Sampler(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("voplay-data-feature-sampler"),
                ..Default::default()
            }))
        }
    })
}

fn binding_resource(resource: &OwnedBindingResource) -> wgpu::BindingResource<'_> {
    match resource {
        OwnedBindingResource::Buffer(buffer) => buffer.as_entire_binding(),
        OwnedBindingResource::Texture { view, .. } => wgpu::BindingResource::TextureView(view),
        OwnedBindingResource::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
    }
}

fn decode_data_feature(bytes: &[u8]) -> Result<DecodedDataFeature<'_>, WgpuSceneRendererError> {
    if bytes.len() < 70 || bytes.get(..6) != Some(b"VRFD2\0") {
        return Err(WgpuSceneRendererError::InvalidConfig);
    }
    let groups = [
        read_u32(bytes, 38)?,
        read_u32(bytes, 42)?,
        read_u32(bytes, 46)?,
        read_u32(bytes, 50)?,
    ];
    let binding_count = read_u32(bytes, 62)? as usize;
    if binding_count > 256 {
        return Err(WgpuSceneRendererError::Capacity);
    }
    let mut offset = 66_usize;
    let mut bindings = Vec::with_capacity(binding_count);
    for _ in 0..binding_count {
        let group = read_u32(bytes, offset)?;
        let binding = read_u32(bytes, offset + 4)?;
        let kind = match *bytes
            .get(offset + 8)
            .ok_or(WgpuSceneRendererError::InvalidConfig)?
        {
            1 => ShaderBindingKind::Uniform,
            2 => ShaderBindingKind::StorageRead,
            3 => ShaderBindingKind::StorageReadWrite,
            4 => ShaderBindingKind::Texture,
            5 => ShaderBindingKind::Sampler,
            _ => return Err(WgpuSceneRendererError::InvalidConfig),
        };
        if bytes.get(offset + 9..offset + 12) != Some(&[0; 3]) {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        let min_size = read_u64(bytes, offset + 12)?;
        bindings.push(DecodedBinding {
            group,
            binding,
            kind,
            min_size,
        });
        offset = offset
            .checked_add(20)
            .ok_or(WgpuSceneRendererError::Capacity)?;
    }
    let wgsl_len = read_u32(bytes, offset)? as usize;
    offset += 4;
    let wgsl_end = offset
        .checked_add(wgsl_len)
        .filter(|end| *end <= bytes.len())
        .ok_or(WgpuSceneRendererError::InvalidConfig)?;
    let wgsl = std::str::from_utf8(&bytes[offset..wgsl_end])
        .map_err(|_| WgpuSceneRendererError::InvalidConfig)?;
    offset = wgsl_end;
    let material_len = read_u32(bytes, offset)? as usize;
    offset += 4;
    let end = offset
        .checked_add(material_len)
        .filter(|end| *end == bytes.len())
        .ok_or(WgpuSceneRendererError::InvalidConfig)?;
    Ok(DecodedDataFeature {
        wgsl,
        material_defaults: &bytes[offset..end],
        sample_count: read_u32(bytes, 26)?,
        groups,
        bindings,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, WgpuSceneRendererError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(WgpuSceneRendererError::InvalidConfig)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| WgpuSceneRendererError::InvalidConfig)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, WgpuSceneRendererError> {
    bytes
        .get(offset..offset + 8)
        .ok_or(WgpuSceneRendererError::InvalidConfig)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| WgpuSceneRendererError::InvalidConfig)
}
