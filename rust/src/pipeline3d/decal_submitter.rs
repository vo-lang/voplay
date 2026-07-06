use std::sync::atomic::{AtomicUsize, Ordering};

static PREPARED_DECAL_REPORT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecalSubmitter;

impl DecalSubmitter {
    pub(crate) fn prepare(decals: &[crate::pipeline_post::PostDecalGpu]) -> usize {
        let prepared = decals.len();
        Self::upload_prepared_decal_report(prepared);
        prepared
    }

    pub(crate) fn upload_prepared_decal_report(count: usize) {
        PREPARED_DECAL_REPORT.store(count, Ordering::Relaxed);
    }
}
