#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WaterSubmitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WaterSubmitPlan {
    pub(crate) filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    pub(crate) owner: &'static str,
    pub(crate) report: &'static str,
}

impl WaterSubmitter {
    pub(crate) fn draw() -> WaterSubmitPlan {
        WaterSubmitPlan {
            filter: crate::primitive_pipeline::PrimitiveRenderFilter::Water,
            owner: "WaterSubmitter",
            report: Self::report_water_draw(),
        }
    }

    pub(crate) fn report_water_draw() -> &'static str {
        "water"
    }
}
