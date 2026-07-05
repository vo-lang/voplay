#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitter;

impl PrimitiveSubmitter {
    pub(crate) fn draw(
        filter: crate::primitive_pipeline::PrimitiveRenderFilter,
    ) -> crate::primitive_pipeline::PrimitiveRenderFilter {
        filter
    }
}
