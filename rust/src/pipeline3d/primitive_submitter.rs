#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitPlan {
    pub(crate) filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    pub(crate) owner: &'static str,
    pub(crate) report: &'static str,
}

impl PrimitiveSubmitter {
    pub(crate) fn draw(
        filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    ) -> PrimitiveSubmitPlan {
        PrimitiveSubmitPlan {
            filter,
            owner: "PrimitiveSubmitter",
            report: Self::report_draw_filter(filter),
        }
    }

    pub(crate) fn report_draw_filter(
        filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    ) -> &'static str {
        match filter {
            crate::primitive_pipeline::PrimitiveRenderFilter::Main => "main",
            crate::primitive_pipeline::PrimitiveRenderFilter::Translucent => "translucent",
            crate::primitive_pipeline::PrimitiveRenderFilter::Water => "water",
        }
    }
}
