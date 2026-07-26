use voplay_runtime::render_feature::{
    encode_wgsl_feature_node, FeatureError, FeatureFactory, FeatureFactoryId, FeatureId,
    PipelineSpecialization, RenderFeatureDescriptor,
};

pub const BUILTIN_OVERLAY_FEATURE_ID: FeatureId = FeatureId(0x76_6f_70_6c_61_79_30_01);
pub const BUILTIN_OVERLAY_FACTORY_ID: FeatureFactoryId =
    FeatureFactoryId(0x76_6f_70_6c_61_79_f0_01);
pub const BUILTIN_OVERLAY_FACTORY_VERSION: u32 = 1;
pub const BUILTIN_OVERLAY_SHADER_HASH: [u8; 32] = [
    0xbb, 0x8f, 0x4c, 0xa4, 0xfb, 0x29, 0x81, 0x1c, 0x8c, 0x27, 0xe9, 0xd6, 0xf8, 0xf4, 0x2a, 0x2e,
    0x4e, 0xb8, 0x5f, 0x83, 0x99, 0x91, 0x45, 0x98, 0x4b, 0x0c, 0xec, 0x21, 0xe3, 0x5f, 0x08, 0x41,
];

pub fn builtin_overlay_feature_factory() -> Box<dyn FeatureFactory> {
    Box::new(BuiltinOverlayFeatureFactory)
}

struct BuiltinOverlayFeatureFactory;

impl FeatureFactory for BuiltinOverlayFeatureFactory {
    fn id(&self) -> FeatureFactoryId {
        BUILTIN_OVERLAY_FACTORY_ID
    }

    fn version(&self) -> u32 {
        BUILTIN_OVERLAY_FACTORY_VERSION
    }

    fn feature(&self) -> FeatureId {
        BUILTIN_OVERLAY_FEATURE_ID
    }

    fn realize(
        &mut self,
        descriptor: &RenderFeatureDescriptor,
        specialization: &PipelineSpecialization,
    ) -> Result<Vec<u8>, FeatureError> {
        if descriptor.id != BUILTIN_OVERLAY_FEATURE_ID
            || descriptor.shader_hash != BUILTIN_OVERLAY_SHADER_HASH
            || !descriptor.shader_abi.bindings.is_empty()
        {
            return Err(FeatureError::FactoryRejected);
        }
        encode_wgsl_feature_node(descriptor, specialization, BUILTIN_OVERLAY_WGSL, &[])
    }
}

const BUILTIN_OVERLAY_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = positions[vertex_index];
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = position * 0.5 + vec2<f32>(0.5);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let major = step(0.985, fract(input.uv.x * 16.0));
    let minor = step(0.985, fract(input.uv.y * 16.0));
    let alpha = max(major, minor) * 0.18;
    return vec4<f32>(0.15, 0.75, 1.0, alpha);
}
"#;
