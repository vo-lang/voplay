#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WaterSubmitter;

impl WaterSubmitter {
    pub(crate) fn draw() -> crate::primitive_pipeline::PrimitiveRenderFilter {
        crate::primitive_pipeline::PrimitiveRenderFilter::Water
    }
}
