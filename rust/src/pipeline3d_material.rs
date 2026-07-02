use crate::material::MaterialSamplerKey;
use crate::model_loader::MeshMaterial;

#[derive(Clone, Copy, Debug)]
pub struct MaterialOverride {
    pub id: u32,
    pub base_color: [f32; 4],
    pub albedo_texture_id: u32,
    pub normal_texture_id: u32,
    pub metallic_roughness_texture_id: u32,
    pub emissive_texture_id: u32,
    pub mask_texture_id: u32,
    pub emissive_color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    pub normal_scale: f32,
    pub uv_scale: f32,
    pub toon_ramp_texture_id: u32,
    pub detail_strength: f32,
    pub macro_blend: f32,
    pub roughness_response: f32,
    pub toon_ramp_response: f32,
    pub shading_mode: u32,
    pub wrap_mode: u32,
    pub filter_mode: u32,
}

impl Default for MaterialOverride {
    fn default() -> Self {
        Self {
            id: 0,
            base_color: [1.0, 1.0, 1.0, 1.0],
            albedo_texture_id: 0,
            normal_texture_id: 0,
            metallic_roughness_texture_id: 0,
            emissive_texture_id: 0,
            mask_texture_id: 0,
            emissive_color: [0.0, 0.0, 0.0, 0.0],
            roughness: 0.55,
            metallic: 0.0,
            normal_scale: 0.0,
            uv_scale: 1.0,
            toon_ramp_texture_id: 0,
            detail_strength: 1.0,
            macro_blend: 0.0,
            roughness_response: 1.0,
            toon_ramp_response: 1.0,
            shading_mode: 0,
            wrap_mode: 0,
            filter_mode: 0,
        }
    }
}

impl MaterialOverride {
    pub fn base_color_multiplier(&self) -> [f32; 4] {
        let base = self.base_color;
        if base == [0.0, 0.0, 0.0, 0.0] && self.id == 0 {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            base
        }
    }

    pub fn uv_scale_or(&self, fallback: f32) -> f32 {
        if self.uv_scale > 0.0 {
            self.uv_scale
        } else {
            fallback
        }
    }

    pub(crate) fn roughness_value(&self, fallback: f32) -> f32 {
        if self.roughness > 0.0 {
            self.roughness.clamp(0.04, 1.0)
        } else {
            fallback.clamp(0.04, 1.0)
        }
    }

    pub(crate) fn metallic_value(&self, fallback: f32) -> f32 {
        if self.metallic != 0.0 {
            self.metallic.clamp(0.0, 1.0)
        } else {
            fallback.clamp(0.0, 1.0)
        }
    }

    pub(crate) fn normal_scale_value(&self, fallback: f32) -> f32 {
        if self.normal_scale > 0.0 {
            self.normal_scale
        } else if self.normal_texture_id != 0 {
            1.0
        } else {
            fallback.max(0.0)
        }
    }

    pub(crate) fn shading_mode_value(&self) -> f32 {
        if self.shading_mode == 0 {
            0.0
        } else {
            self.shading_mode as f32
        }
    }

    pub(crate) fn sampler_key(&self, fallback: MaterialSamplerKey) -> MaterialSamplerKey {
        MaterialSamplerKey::resolve(self.wrap_mode, self.filter_mode, fallback)
    }

    pub(crate) fn mesh_material_params(
        &self,
        uv_scale: f32,
        roughness: f32,
        metallic: f32,
    ) -> [f32; 4] {
        [
            self.uv_scale_or(uv_scale),
            self.roughness_value(roughness),
            self.metallic_value(metallic),
            self.shading_mode_value(),
        ]
    }

    pub(crate) fn material_response_values(&self, mesh_material: &MeshMaterial) -> [f32; 4] {
        [
            if self.detail_strength > 0.0 {
                self.detail_strength
            } else if mesh_material.detail_strength > 0.0 {
                mesh_material.detail_strength
            } else {
                1.0
            },
            if self.macro_blend > 0.0 {
                self.macro_blend
            } else {
                mesh_material.macro_blend.max(0.0)
            },
            if self.roughness_response > 0.0 {
                self.roughness_response
            } else if mesh_material.roughness_response > 0.0 {
                mesh_material.roughness_response
            } else {
                1.0
            },
            if self.toon_ramp_response > 0.0 {
                self.toon_ramp_response
            } else if mesh_material.toon_ramp_response > 0.0 {
                mesh_material.toon_ramp_response
            } else {
                1.0
            },
        ]
    }

    pub(crate) fn emissive_color_value(
        &self,
        source_factor: [f32; 3],
        override_texture_active: bool,
    ) -> [f32; 4] {
        let e = self.emissive_color;
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
}
