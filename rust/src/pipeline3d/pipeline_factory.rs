use super::*;

pub(crate) struct PipelineFactory;

impl PipelineFactory {
    pub(crate) fn create(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        receiver_mask_format: wgpu::TextureFormat,
        surface_props_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Pipeline3D {
        let static_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh"),
            source: wgpu::ShaderSource::Wgsl(STATIC_MESH_SHADER.into()),
        });
        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh_terrain"),
            source: wgpu::ShaderSource::Wgsl(TERRAIN_MESH_SHADER.into()),
        });
        let skinned_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh_skinned"),
            source: wgpu::ShaderSource::Wgsl(SKINNED_MESH_SHADER.into()),
        });

        // Group 0: Camera
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_camera_bgl"),
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

        // Group 1: Model transform (dynamic offset for per-draw uniforms)
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_model_bgl"),
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
        let model_buffer_alignment = device.limits().min_uniform_buffer_offset_alignment;

        // Group 2: Lights
        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_light_bgl"),
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
                label: Some("voplay_mesh_main_texture_bgl"),
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
        let mut terrain_texture_entries = vec![
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
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        for binding in 4..16 {
            terrain_texture_entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        terrain_texture_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 16,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let terrain_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay_mesh_terrain_texture_bgl"),
                entries: &terrain_texture_entries,
            });

        // Group 3: Main texture + shadow map
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_mesh_layout"),
            bind_group_layouts: &[
                &camera_bgl,
                &model_bgl,
                &light_bgl,
                &main_texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let terrain_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("voplay_mesh_terrain_layout"),
                bind_group_layouts: &[
                    &camera_bgl,
                    &model_bgl,
                    &light_bgl,
                    &terrain_texture_bind_group_layout,
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
        let multisample = wgpu::MultisampleState {
            count: sample_count,
            ..wgpu::MultisampleState::default()
        };

        let instanced_vertex_state = wgpu::VertexState {
            module: &static_shader,
            entry_point: Some("vs_instanced"),
            buffers: &[MeshVertex::layout(), InstanceData::layout()],
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

        let instanced_textured_targets = color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let pipeline_instanced_textured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_textured"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state.clone(),
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced"),
                    targets: &instanced_textured_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });
        let instanced_textured_color_targets =
            color_only_targets(surface_format, Some(wgpu::BlendState::ALPHA_BLENDING));
        let pipeline_instanced_textured_color =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_textured_color"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state.clone(),
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced_color"),
                    targets: &instanced_textured_color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });

        let terrain_targets = color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let pipeline_terrain_splat =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_terrain_splat"),
                layout: Some(&terrain_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &terrain_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &terrain_shader,
                    entry_point: Some("fs_main_terrain"),
                    targets: &terrain_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });
        let terrain_color_targets =
            color_only_targets(surface_format, Some(wgpu::BlendState::ALPHA_BLENDING));
        let pipeline_terrain_splat_color =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_terrain_splat_color"),
                layout: Some(&terrain_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &terrain_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &terrain_shader,
                    entry_point: Some("fs_main_terrain_color"),
                    targets: &terrain_color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });

        let skinned_textured_targets = color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let pipeline_skinned_textured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_textured"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned"),
                    targets: &skinned_textured_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });
        let skinned_textured_color_targets =
            color_only_targets(surface_format, Some(wgpu::BlendState::ALPHA_BLENDING));
        let pipeline_skinned_textured_color =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_textured_color"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned_color"),
                    targets: &skinned_textured_color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });

        let skinned_untextured_targets = color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let pipeline_skinned_untextured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_untextured"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned_no_tex"),
                    targets: &skinned_untextured_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });
        let skinned_untextured_color_targets =
            color_only_targets(surface_format, Some(wgpu::BlendState::ALPHA_BLENDING));
        let pipeline_skinned_untextured_color =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_untextured_color"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned_no_tex_color"),
                    targets: &skinned_untextured_color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });

        let instanced_untextured_targets = color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let pipeline_instanced_untextured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_untextured"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state.clone(),
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced_no_tex"),
                    targets: &instanced_untextured_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });
        let instanced_untextured_color_targets =
            color_only_targets(surface_format, Some(wgpu::BlendState::ALPHA_BLENDING));
        let pipeline_instanced_untextured_color =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_untextured_color"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state,
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced_no_tex_color"),
                    targets: &instanced_untextured_color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample,
                multiview: None,
                cache: None,
            });

        // Create uniform buffers
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_camera_ub"),
            size: std::mem::size_of::<Camera3DUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let model_buffer_slot_count: u32 = 256;
        let aligned_model_size = Pipeline3D::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            model_buffer_alignment,
        );
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_model_ub"),
            size: aligned_model_size as u64 * model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = Pipeline3D::create_model_bind_group(
            device,
            &model_bgl,
            &model_buffer,
            std::mem::size_of::<ModelUniform>() as u64,
            "voplay_mesh_model_bg",
        );

        let skinned_model_buffer_slot_count: u32 = 32;
        let aligned_skinned_size = Pipeline3D::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            model_buffer_alignment,
        );
        let skinned_model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_skinned_model_ub"),
            size: aligned_skinned_size as u64 * skinned_model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skinned_model_bind_group = Pipeline3D::create_model_bind_group(
            device,
            &model_bgl,
            &skinned_model_buffer,
            std::mem::size_of::<SkinnedModelUniform>() as u64,
            "voplay_mesh_skinned_model_bg",
        );

        let instance_buffer_capacity: u32 = 1024;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_instance_vb"),
            size: std::mem::size_of::<InstanceData>() as u64 * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_light_ub"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_light_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
        });

        let material_samplers = MATERIAL_SAMPLER_KEYS
            .iter()
            .map(|key| Pipeline3D::create_material_sampler(device, *key))
            .collect();
        let material_clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_material_sampler_clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        // 1x1 white texture for untextured meshes
        let white_data = [255u8; 4]; // RGBA white
        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_white_1x1"),
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
        let white_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        Pipeline3D {
            pipeline_instanced_textured,
            pipeline_instanced_textured_color,
            pipeline_instanced_untextured,
            pipeline_instanced_untextured_color,
            pipeline_terrain_splat,
            pipeline_terrain_splat_color,
            pipeline_skinned_textured,
            pipeline_skinned_textured_color,
            pipeline_skinned_untextured,
            pipeline_skinned_untextured_color,
            camera_buffer,
            camera_bind_group,
            model_bgl,
            model_buffer,
            model_bind_group,
            model_buffer_alignment,
            model_buffer_slot_count,
            skinned_model_buffer,
            skinned_model_bind_group,
            skinned_model_buffer_slot_count,
            instance_buffer,
            instance_buffer_capacity,
            light_buffer,
            light_bind_group,
            main_texture_bind_group_layout,
            terrain_texture_bind_group_layout,
            material_samplers,
            material_clamp_sampler,
            white_texture_view: white_view,
            main_texture_bind_groups: HashMap::new(),
            terrain_texture_bind_groups: HashMap::new(),
        }
    }
}
