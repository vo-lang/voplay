#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitter;

impl PrimitiveSubmitter {
    pub(crate) fn draw(
        filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    ) -> crate::primitive_pipeline::PrimitiveRenderFilter {
        Self::report_draw_filter(filter);
        filter
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
