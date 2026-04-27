pub const MATERIAL_WRAP_SOURCE: u32 = 0;
pub const MATERIAL_WRAP_REPEAT: u32 = 1;
pub const MATERIAL_WRAP_CLAMP: u32 = 2;
pub const MATERIAL_WRAP_MIRROR: u32 = 3;

pub const MATERIAL_FILTER_SOURCE: u32 = 0;
pub const MATERIAL_FILTER_LINEAR: u32 = 1;
pub const MATERIAL_FILTER_NEAREST: u32 = 2;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct MaterialSamplerKey {
    pub wrap_mode: u32,
    pub filter_mode: u32,
}

impl MaterialSamplerKey {
    pub const REPEAT_LINEAR: Self = Self {
        wrap_mode: MATERIAL_WRAP_REPEAT,
        filter_mode: MATERIAL_FILTER_LINEAR,
    };

    pub fn resolve(wrap_mode: u32, filter_mode: u32, fallback: Self) -> Self {
        Self {
            wrap_mode: match wrap_mode {
                MATERIAL_WRAP_SOURCE => fallback.wrap_mode,
                MATERIAL_WRAP_REPEAT | MATERIAL_WRAP_CLAMP | MATERIAL_WRAP_MIRROR => wrap_mode,
                _ => fallback.wrap_mode,
            },
            filter_mode: match filter_mode {
                MATERIAL_FILTER_SOURCE => fallback.filter_mode,
                MATERIAL_FILTER_LINEAR | MATERIAL_FILTER_NEAREST => filter_mode,
                _ => fallback.filter_mode,
            },
        }
    }

    pub fn sampler_index(self) -> usize {
        let wrap_index = match self.wrap_mode {
            MATERIAL_WRAP_REPEAT => 0,
            MATERIAL_WRAP_CLAMP => 1,
            MATERIAL_WRAP_MIRROR => 2,
            _ => 0,
        };
        let filter_index = match self.filter_mode {
            MATERIAL_FILTER_NEAREST => 1,
            _ => 0,
        };
        filter_index * 3 + wrap_index
    }
}

pub const MATERIAL_SAMPLER_KEYS: [MaterialSamplerKey; 6] = [
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_REPEAT,
        filter_mode: MATERIAL_FILTER_LINEAR,
    },
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_CLAMP,
        filter_mode: MATERIAL_FILTER_LINEAR,
    },
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_MIRROR,
        filter_mode: MATERIAL_FILTER_LINEAR,
    },
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_REPEAT,
        filter_mode: MATERIAL_FILTER_NEAREST,
    },
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_CLAMP,
        filter_mode: MATERIAL_FILTER_NEAREST,
    },
    MaterialSamplerKey {
        wrap_mode: MATERIAL_WRAP_MIRROR,
        filter_mode: MATERIAL_FILTER_NEAREST,
    },
];
