use crate::material::MaterialSamplerKey;
use crate::model_loader::ModelId;
use crate::pipeline3d::ModelUniform;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct InstanceData {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
    base_color: [f32; 4],
    material_params: [f32; 4],
    emissive_color: [f32; 4],
    texture_flags: [f32; 4],
    material_response: [f32; 4],
    texture_flags2: [f32; 4],
    instance_params: [f32; 4],
}

impl InstanceData {
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

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }

    pub(crate) fn from_uniform(uniform: &ModelUniform) -> Self {
        Self {
            model_0: uniform.model[0],
            model_1: uniform.model[1],
            model_2: uniform.model[2],
            model_3: uniform.model[3],
            base_color: uniform.base_color,
            material_params: uniform.material_params,
            emissive_color: uniform.emissive_color,
            texture_flags: uniform.texture_flags,
            material_response: uniform.material_response,
            texture_flags2: uniform.texture_flags2,
            instance_params: [0.0; 4],
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct MainTextureKey {
    pub(crate) albedo: u32,
    pub(crate) normal: u32,
    pub(crate) metallic_roughness: u32,
    pub(crate) emissive: u32,
    pub(crate) toon_ramp: u32,
    pub(crate) mask: u32,
    pub(crate) sampler: MaterialSamplerKey,
}

impl MainTextureKey {
    pub(crate) fn has_albedo(self) -> bool {
        self.albedo != 0
    }

    pub(crate) fn texture_flags(self, normal_scale: f32) -> [f32; 4] {
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

    pub(crate) fn texture_flags2(self) -> [f32; 4] {
        [if self.mask != 0 { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct InstanceBatchKey {
    pub(crate) model_id: ModelId,
    pub(crate) mesh_index: usize,
    pub(crate) textures: MainTextureKey,
}

pub(crate) struct InstanceBatch {
    pub(crate) key: InstanceBatchKey,
    pub(crate) instances: Vec<InstanceData>,
}

pub(crate) struct InstanceBatchDraw {
    pub(crate) key: InstanceBatchKey,
    pub(crate) start: u32,
    pub(crate) count: u32,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct TerrainTextureKey {
    pub(crate) control: u32,
    pub(crate) albedo_layers: [u32; 4],
    pub(crate) normal_layers: [u32; 4],
    pub(crate) metallic_roughness_layers: [u32; 4],
    pub(crate) material: [u32; 16],
}

pub(crate) struct TerrainBindGroupEntry {
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) _material_buffer: wgpu::Buffer,
}
