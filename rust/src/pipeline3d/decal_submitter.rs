#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecalSubmitter;

impl DecalSubmitter {
    pub(crate) fn prepare(decals: &[crate::pipeline_post::PostDecalGpu]) -> usize {
        decals.len()
    }
}
