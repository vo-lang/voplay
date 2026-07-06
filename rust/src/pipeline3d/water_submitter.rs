#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WaterSubmitter;

impl WaterSubmitter {
    pub(crate) fn draw() -> crate::primitive_pipeline::PrimitiveRenderFilter {
        Self::report_water_draw();
        crate::primitive_pipeline::PrimitiveRenderFilter::Water
    }

    pub(crate) fn report_water_draw() -> &'static str {
        "water"
    }
}
