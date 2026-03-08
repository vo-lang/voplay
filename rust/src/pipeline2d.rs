//! 2D shape rendering pipeline — instanced quads for rects, circles, lines.
//! Uses a single wgpu pipeline with per-instance shape type dispatched in the fragment shader.

use bytemuck::{Pod, Zeroable};

// Shape type constants (matched in WGSL shader).
#[allow(dead_code)] const SHAPE_RECT: f32 = 0.0;
#[allow(dead_code)] const SHAPE_CIRCLE: f32 = 1.0;
#[allow(dead_code)] const SHAPE_LINE: f32 = 2.0;

/// Vertex for the unit quad.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct QuadVertex {
    pub position: [f32; 2],
}

impl QuadVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Unit quad vertices: two triangles forming a [0,0]→[1,1] quad.
pub const QUAD_VERTICES: [QuadVertex; 6] = [
    QuadVertex { position: [0.0, 0.0] },
    QuadVertex { position: [1.0, 0.0] },
    QuadVertex { position: [0.0, 1.0] },
    QuadVertex { position: [1.0, 0.0] },
    QuadVertex { position: [1.0, 1.0] },
    QuadVertex { position: [0.0, 1.0] },
];

/// Per-instance data for a 2D shape.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShapeInstance {
    pub rect: [f32; 4],   // x, y, w, h
    pub color: [f32; 4],  // RGBA
    pub params: [f32; 4], // shape_type, rotation, corner_radius, _unused
}

impl ShapeInstance {
    const ATTRIBS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        1 => Float32x4,  // rect
        2 => Float32x4,  // color
        3 => Float32x4,  // params
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Camera uniform — orthographic projection matrix.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CameraUniform {
    pub projection: [[f32; 4]; 4],
}

impl CameraUniform {
    /// Screen-space orthographic projection: (0,0) top-left, (w,h) bottom-right.
    pub fn screen_space(width: f32, height: f32) -> Self {
        Self {
            projection: crate::math3d::orthographic(0.0, width, height, 0.0, -1.0, 1.0),
        }
    }

    /// World-space projection with 2D camera transform.
    pub fn with_camera(
        width: f32,
        height: f32,
        cam_x: f32,
        cam_y: f32,
        zoom: f32,
        rotation: f32,
    ) -> Self {
        // Camera transform: center at (cam_x, cam_y), apply zoom and rotation.
        // 1. Translate so camera center is at origin
        // 2. Rotate by -rotation
        // 3. Scale by zoom
        // 4. Translate so origin maps to screen center
        // 5. Apply screen-space orthographic projection
        //
        // Combined into a single matrix for the shader.

        let cos_r = rotation.cos();
        let sin_r = rotation.sin();
        let z = zoom;

        // Row-major, then transposed to column-major for wgpu.
        // Projection * Translate(hw, hh) * Scale(zoom) * Rotate(-rot) * Translate(-cam_x, -cam_y)
        //
        // ortho(0,w,h,0,-1,1) maps screen-space to clip:
        //   x' = 2x/w - 1
        //   y' = -(2y/h - 1) = 1 - 2y/h
        //
        // Full combined:
        //   world → screen:
        //     sx = (wx - cam_x) * cos_r * z + (wy - cam_y) * sin_r * z + hw
        //     sy = -(wx - cam_x) * sin_r * z + (wy - cam_y) * cos_r * z + hh
        //   screen → clip:
        //     cx = 2*sx/w - 1
        //     cy = 1 - 2*sy/h

        let a = 2.0 * z * cos_r / width;
        let b = 2.0 * z * sin_r / width;
        let c = -2.0 * z * sin_r / height;
        let d = -2.0 * z * cos_r / height;

        let tx = -a * cam_x - b * cam_y;
        let ty = -c * cam_x - d * cam_y;

        // Column-major storage for wgpu
        Self {
            projection: [
                [a, c, 0.0, 0.0],
                [b, d, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [tx, ty, 0.0, 1.0],
            ],
        }
    }
}


/// The full 2D pipeline state.
pub struct Pipeline2D {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

const INITIAL_INSTANCE_CAPACITY: usize = 1024;

impl Pipeline2D {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // Shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_shape2d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shape2d.wgsl").into()),
        });

        // Pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_shape2d_layout"),
            bind_group_layouts: &[camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Render pipeline — alpha blending, no depth write (2D).
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shape2d"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[QuadVertex::layout(), ShapeInstance::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None, // No culling for 2D
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: false, // 2D: no depth write
                depth_compare: wgpu::CompareFunction::Always, // 2D: always pass
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Vertex buffer (static unit quad — uploaded once at creation)
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_quad_vb"),
            size: std::mem::size_of_val(&QUAD_VERTICES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&QUAD_VERTICES));

        // Instance buffer (dynamic, resized as needed)
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shape_ib"),
            size: (INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<ShapeInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
        }
    }

    /// Upload pre-sorted shape instances to the GPU buffer.
    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[ShapeInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        if instances.len() > self.instance_capacity {
            let new_capacity = instances.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("voplay_shape_ib"),
                size: (new_capacity * std::mem::size_of::<ShapeInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_capacity;
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
    }

    /// Draw a sub-range of the uploaded instance buffer.
    pub fn draw_range<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        camera_offset: &[u32],
        start: u32,
        count: u32,
    ) {
        if count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bind_group, camera_offset);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        pass.draw(0..6, start..start + count);
    }
}
