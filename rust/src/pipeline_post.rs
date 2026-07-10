pub const MAX_POST_DECALS: usize = 32;
// The post pass also samples the source, depth, receiver mask and surface props
// textures, so keep atlas bindings under the portable WebGPU sampled-texture limit.
pub const MAX_POST_DECAL_ATLASES: usize = 2;

const POST_DECAL_FLAG_NORMAL_ATLAS: f32 = 1.0;
const POST_DECAL_FLAG_ROUGHNESS_ATLAS: f32 = 2.0;
const POST_DECAL_FLAG_MASK_ATLAS: f32 = 4.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostUniform {
    params0: [f32; 4],
    params1: [f32; 4],
    params2: [f32; 4],
    params3: [f32; 4],
    params4: [f32; 4],
    params5: [f32; 4],
    params6: [f32; 4],
    params7: [f32; 4],
    params8: [f32; 4],
    params9: [f32; 4],
    params10: [f32; 4],
}

impl PostUniform {
    pub fn for_size(width: u32, height: u32) -> Self {
        Self::from_settings(
            width, height, 0.74, 0.105, 0.055, 0.82, 0.0, 2.5, 70.0, 0.18, 0.95, 0.015, 2,
        )
    }

    pub fn from_settings(
        width: u32,
        height: u32,
        bloom_threshold: f32,
        bloom_strength: f32,
        sharpen_strength: f32,
        fxaa_strength: f32,
        contact_ao_strength: f32,
        contact_ao_radius: f32,
        contact_ao_depth_scale: f32,
        contact_ao_detail_strength: f32,
        contact_ao_detail_radius: f32,
        contact_ao_normal_bias: f32,
        contact_ao_quality: u32,
    ) -> Self {
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        Self {
            // texel_size.xy, bloom threshold, bloom strength
            params0: [1.0 / width, 1.0 / height, bloom_threshold, bloom_strength],
            // sharpen strength, FXAA strength, reserved, reserved
            params1: [sharpen_strength, fxaa_strength, 0.0, 0.0],
            // contact AO strength, radius in pixels, depth response scale, reserved
            params2: [
                contact_ao_strength.clamp(0.0, 1.5),
                contact_ao_radius.clamp(0.5, 8.0),
                contact_ao_depth_scale.clamp(1.0, 400.0),
                0.0,
            ],
            // primary presentation light direction for post decal material response.
            params3: [-0.42, 0.82, 0.32, 1.0],
            // contact AO detail strength, detail radius, normal bias, reserved.
            params4: [
                contact_ao_detail_strength.clamp(0.0, 1.0),
                contact_ao_detail_radius.clamp(0.35, 3.0),
                contact_ao_normal_bias.clamp(0.0, 0.08),
                contact_ao_quality.min(4) as f32,
            ],
            // secondary and tertiary decal light directions.
            params5: [0.0, 0.0, 0.0, 0.0],
            params6: [0.0, 0.0, 0.0, 0.0],
            // decal light count, decal ambient floor, reserved, reserved.
            params7: [1.0, 0.18, 0.0, 0.0],
            // decal light color rgb, type (0=directional, 1=point).
            params8: [1.0, 1.0, 1.0, 0.0],
            params9: [0.0, 0.0, 0.0, 0.0],
            params10: [0.0, 0.0, 0.0, 0.0],
        }
    }

    pub fn with_decal_lights(mut self, vectors: &[[f32; 4]], colors_types: &[[f32; 4]]) -> Self {
        let mut normalized = [[0.0f32; 4]; 3];
        let mut color_type = [[0.0f32; 4]; 3];
        let mut count = 0usize;
        for (index, light) in vectors.iter().take(3).enumerate() {
            let direction = [light[0], light[1], light[2]];
            let intensity = light[3].max(0.0);
            if intensity <= 0.0 {
                continue;
            }
            let mut light_type = colors_types.get(index).map(|value| value[3]).unwrap_or(0.0);
            light_type = if light_type >= 0.5 { 1.0 } else { 0.0 };
            let mut vector = direction;
            if light_type < 0.5 {
                let len_sq = direction[0] * direction[0]
                    + direction[1] * direction[1]
                    + direction[2] * direction[2];
                if len_sq <= 0.000001 {
                    continue;
                }
                let inv_len = len_sq.sqrt().recip();
                vector = [
                    direction[0] * inv_len,
                    direction[1] * inv_len,
                    direction[2] * inv_len,
                ];
            }
            let color = colors_types
                .get(index)
                .map(|value| [value[0], value[1], value[2]])
                .unwrap_or([1.0, 1.0, 1.0]);
            normalized[count] = [vector[0], vector[1], vector[2], intensity];
            color_type[count] = [
                color[0].max(0.0),
                color[1].max(0.0),
                color[2].max(0.0),
                light_type,
            ];
            count += 1;
            if count >= normalized.len() {
                break;
            }
        }
        if count > 0 {
            self.params3 = normalized[0];
            self.params5 = normalized[1];
            self.params6 = normalized[2];
            self.params7[0] = count as f32;
            self.params8 = color_type[0];
            self.params9 = color_type[1];
            self.params10 = color_type[2];
        }
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostDecalGpu {
    center_width: [f32; 4],
    right_opacity: [f32; 4],
    forward_length: [f32; 4],
    color_depth: [f32; 4],
    uv_rect: [f32; 4],
    atlas_params: [f32; 4],
    material_params: [f32; 4],
    angle_params: [f32; 4],
}

impl PostDecalGpu {
    pub fn new(
        position: [f32; 3],
        yaw: f32,
        width: f32,
        length: f32,
        depth: f32,
        color: [f32; 4],
    ) -> Self {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let right = [cos_yaw, 0.0, -sin_yaw];
        let forward = [sin_yaw, 0.0, cos_yaw];
        Self {
            center_width: [
                position[0],
                position[1],
                position[2],
                (width * 0.5).max(0.001),
            ],
            right_opacity: [right[0], right[1], right[2], color[3].clamp(0.0, 1.0)],
            forward_length: [
                forward[0],
                forward[1],
                forward[2],
                (length * 0.5).max(0.001),
            ],
            color_depth: [
                color[0].clamp(0.0, 1.0),
                color[1].clamp(0.0, 1.0),
                color[2].clamp(0.0, 1.0),
                depth.max(0.001),
            ],
            uv_rect: [0.0, 0.0, 0.0, 0.0],
            atlas_params: [-1.0, 0.0, 0.0, 3.0],
            material_params: [0.0, 0.72, 0.0, 0.0],
            angle_params: [0.0, 0.0, 0.0, 0.0],
        }
    }

    pub fn new_with_uv(
        position: [f32; 3],
        yaw: f32,
        width: f32,
        length: f32,
        depth: f32,
        color: [f32; 4],
        uv_rect: [f32; 4],
        atlas_slot: Option<u32>,
    ) -> Self {
        let mut decal = Self::new(position, yaw, width, length, depth, color);
        decal.uv_rect = [
            uv_rect[0].clamp(0.0, 1.0),
            uv_rect[1].clamp(0.0, 1.0),
            uv_rect[2].clamp(0.0, 1.0),
            uv_rect[3].clamp(0.0, 1.0),
        ];
        decal.atlas_params[0] = atlas_slot.map(|slot| slot as f32).unwrap_or(-1.0);
        decal
    }

    pub fn with_distance_fade(mut self, start: f32, end: f32) -> Self {
        if start >= 0.0 && end > start {
            self.atlas_params[1] = start;
            self.atlas_params[2] = end;
        }
        self
    }

    pub fn with_receiver_mask(mut self, receivers: u32) -> Self {
        self.atlas_params[3] = receivers.min(3) as f32;
        self
    }

    pub fn with_surface_response(
        mut self,
        normal_strength: f32,
        roughness: f32,
        roughness_strength: f32,
    ) -> Self {
        self.material_params = [
            normal_strength.clamp(0.0, 2.0),
            roughness.clamp(0.04, 1.0),
            roughness_strength.clamp(0.0, 1.0),
            0.0,
        ];
        self
    }

    pub fn with_material_maps(
        mut self,
        normal_atlas: bool,
        roughness_atlas: bool,
        mask_atlas: bool,
    ) -> Self {
        let mut flags = 0.0;
        if normal_atlas {
            flags += POST_DECAL_FLAG_NORMAL_ATLAS;
        }
        if roughness_atlas {
            flags += POST_DECAL_FLAG_ROUGHNESS_ATLAS;
        }
        if mask_atlas {
            flags += POST_DECAL_FLAG_MASK_ATLAS;
        }
        self.material_params[3] = flags;
        self
    }

    pub fn with_angle_fade(mut self, start: f32, end: f32) -> Self {
        if start >= 0.0 && end > start {
            self.angle_params[0] = start.clamp(0.0, 1.0);
            self.angle_params[1] = end.clamp(0.0, 1.0);
        }
        self
    }

    pub(crate) fn render_batch_bounds(&self) -> ([f32; 3], f32) {
        let half_width = self.center_width[3].max(0.001);
        let half_length = self.forward_length[3].max(0.001);
        let depth = self.color_depth[3].max(0.001);
        let radius = (half_width * half_width + half_length * half_length + depth * depth)
            .sqrt()
            .max(0.001);
        (
            [
                self.center_width[0],
                self.center_width[1],
                self.center_width[2],
            ],
            radius,
        )
    }

    pub(crate) fn render_batch_material_group(&self) -> u32 {
        let atlas_slot = self.atlas_params[0];
        let atlas_group = if atlas_slot >= 0.0 {
            atlas_slot as u32 + 1
        } else {
            0
        };
        (atlas_group << 16) ^ self.material_params[3].to_bits()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostDecalUniform {
    inv_view_proj: [[f32; 4]; 4],
    params: [u32; 4],
    camera_pos: [f32; 4],
    decals: [PostDecalGpu; MAX_POST_DECALS],
}

impl PostDecalUniform {
    pub fn empty() -> Self {
        Self {
            inv_view_proj: crate::math3d::MAT4_IDENTITY,
            params: [0, 0, 0, 0],
            camera_pos: [0.0, 0.0, 0.0, 1.0],
            decals: [PostDecalGpu::new([0.0, 0.0, 0.0], 0.0, 1.0, 1.0, 1.0, [0.0, 0.0, 0.0, 0.0]);
                MAX_POST_DECALS],
        }
    }

    pub fn from_decals(
        inv_view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
        decals: &[PostDecalGpu],
        atlas_count: u32,
    ) -> Self {
        let mut uniform = Self::empty();
        uniform.inv_view_proj = inv_view_proj;
        uniform.camera_pos = [camera_pos[0], camera_pos[1], camera_pos[2], 1.0];
        let count = decals.len().min(MAX_POST_DECALS);
        uniform.params[0] = count as u32;
        uniform.params[1] = atlas_count.min(MAX_POST_DECAL_ATLASES as u32);
        uniform.decals[..count].copy_from_slice(&decals[..count]);
        uniform
    }
}

pub struct PipelinePost {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    _decal_fallback_texture: wgpu::Texture,
    decal_fallback_view: wgpu::TextureView,
    _decal_normal_fallback_texture: wgpu::Texture,
    decal_normal_fallback_view: wgpu::TextureView,
    _decal_roughness_fallback_texture: wgpu::Texture,
    decal_roughness_fallback_view: wgpu::TextureView,
    _decal_mask_fallback_texture: wgpu::Texture,
    decal_mask_fallback_view: wgpu::TextureView,
}

impl PipelinePost {
    fn filterable_texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }
    }

    fn create_fallback_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        format: wgpu::TextureFormat,
        rgba: [u8; 4],
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_post_bgl"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PostUniform>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PostDecalUniform>() as u64,
                        ),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                Self::filterable_texture_layout_entry(9),
                Self::filterable_texture_layout_entry(10),
                Self::filterable_texture_layout_entry(11),
                Self::filterable_texture_layout_entry(12),
                Self::filterable_texture_layout_entry(13),
                Self::filterable_texture_layout_entry(14),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_post_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_post_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_post_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let (decal_fallback_texture, decal_fallback_view) = Self::create_fallback_texture(
            device,
            queue,
            "voplay_post_decal_fallback_white",
            wgpu::TextureFormat::Rgba8UnormSrgb,
            [255, 255, 255, 255],
        );
        let (decal_normal_fallback_texture, decal_normal_fallback_view) =
            Self::create_fallback_texture(
                device,
                queue,
                "voplay_post_decal_fallback_normal",
                wgpu::TextureFormat::Rgba8Unorm,
                [128, 128, 255, 255],
            );
        let (decal_roughness_fallback_texture, decal_roughness_fallback_view) =
            Self::create_fallback_texture(
                device,
                queue,
                "voplay_post_decal_fallback_roughness",
                wgpu::TextureFormat::Rgba8Unorm,
                [184, 184, 184, 255],
            );
        let (decal_mask_fallback_texture, decal_mask_fallback_view) = Self::create_fallback_texture(
            device,
            queue,
            "voplay_post_decal_fallback_mask",
            wgpu::TextureFormat::Rgba8Unorm,
            [255, 255, 255, 255],
        );
        Self {
            pipeline,
            bind_group_layout,
            sampler,
            _decal_fallback_texture: decal_fallback_texture,
            decal_fallback_view,
            _decal_normal_fallback_texture: decal_normal_fallback_texture,
            decal_normal_fallback_view,
            _decal_roughness_fallback_texture: decal_roughness_fallback_texture,
            decal_roughness_fallback_view,
            _decal_mask_fallback_texture: decal_mask_fallback_texture,
            decal_mask_fallback_view,
        }
    }

    pub fn decal_fallback_view(&self) -> &wgpu::TextureView {
        &self.decal_fallback_view
    }

    pub fn decal_normal_fallback_view(&self) -> &wgpu::TextureView {
        &self.decal_normal_fallback_view
    }

    pub fn decal_roughness_fallback_view(&self) -> &wgpu::TextureView {
        &self.decal_roughness_fallback_view
    }

    pub fn decal_mask_fallback_view(&self) -> &wgpu::TextureView {
        &self.decal_mask_fallback_view
    }

    pub fn create_uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_post_uniform"),
            size: std::mem::size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn create_decal_uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_post_decal_uniform"),
            size: std::mem::size_of::<PostDecalUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn create_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        camera_depth_view: &wgpu::TextureView,
        uniform_buffer: &wgpu::Buffer,
        decal_uniform_buffer: &wgpu::Buffer,
        decal_atlas_views: [&wgpu::TextureView; MAX_POST_DECAL_ATLASES],
        decal_normal_atlas_views: [&wgpu::TextureView; MAX_POST_DECAL_ATLASES],
        decal_roughness_atlas_views: [&wgpu::TextureView; MAX_POST_DECAL_ATLASES],
        decal_mask_atlas_views: [&wgpu::TextureView; MAX_POST_DECAL_ATLASES],
        receiver_mask_view: &wgpu::TextureView,
        surface_props_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_post_bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(camera_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: decal_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(decal_atlas_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(decal_atlas_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(receiver_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(surface_props_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(decal_normal_atlas_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(decal_normal_atlas_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(decal_roughness_atlas_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(decal_roughness_atlas_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(decal_mask_atlas_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(decal_mask_atlas_views[1]),
                },
            ],
        })
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, bind_group: &'a wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::{PipelinePost, PostDecalUniform, PostUniform, MAX_POST_DECAL_ATLASES};

    #[test]
    fn pipeline_post_creates_with_current_shader_layout() {
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
                label: Some("voplay_post_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let pipeline = PipelinePost::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_post_test_texture"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_post_test_depth"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let receiver_mask_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_post_test_receiver_mask"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let receiver_mask_view =
            receiver_mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let surface_props_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_post_test_surface_props"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let surface_props_view =
            surface_props_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = PipelinePost::create_uniform_buffer(&device);
        let decal_uniform = PipelinePost::create_decal_uniform_buffer(&device);
        let _post_uniform = PostUniform::for_size(4, 4);
        let _post_decal_uniform = PostDecalUniform::empty();
        let fallback_atlas = pipeline.decal_fallback_view();
        let fallback_normal_atlas = pipeline.decal_normal_fallback_view();
        let fallback_roughness_atlas = pipeline.decal_roughness_fallback_view();
        let fallback_mask_atlas = pipeline.decal_mask_fallback_view();
        let _bind_group = pipeline.create_bind_group(
            &device,
            &view,
            &depth_view,
            &uniform,
            &decal_uniform,
            [fallback_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_normal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_roughness_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_mask_atlas; MAX_POST_DECAL_ATLASES],
            &receiver_mask_view,
            &surface_props_view,
        );
    }

    #[test]
    fn post_uniform_accepts_multiple_decal_lights() {
        let uniform = PostUniform::for_size(64, 32).with_decal_lights(
            &[
                [0.0, 2.0, 0.0, 0.8],
                [4.0, 2.0, 0.0, 0.25],
                [0.0, 0.0, -2.0, 0.1],
                [0.0, 1.0, 0.0, 1.0],
            ],
            &[
                [1.0, 0.8, 0.6, 0.0],
                [0.3, 0.5, 1.0, 1.0],
                [0.8, 1.0, 0.4, 0.0],
                [1.0, 1.0, 1.0, 0.0],
            ],
        );
        assert_eq!(uniform.params7[0], 3.0);
        assert!((uniform.params3[1] - 1.0).abs() < 0.00001);
        assert_eq!(uniform.params5[0], 4.0);
        assert_eq!(uniform.params5[1], 2.0);
        assert!((uniform.params6[2] + 1.0).abs() < 0.00001);
        assert_eq!(uniform.params3[3], 0.8);
        assert_eq!(uniform.params5[3], 0.25);
        assert_eq!(uniform.params6[3], 0.1);
        assert_eq!(uniform.params8, [1.0, 0.8, 0.6, 0.0]);
        assert_eq!(uniform.params9, [0.3, 0.5, 1.0, 1.0]);
        assert_eq!(uniform.params10, [0.8, 1.0, 0.4, 0.0]);
    }
}
