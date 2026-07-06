use std::sync::atomic::{AtomicUsize, Ordering};

static PREPARED_DECAL_REPORT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecalSubmitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecalSubmitPlan {
    pub(crate) prepared_count: usize,
    pub(crate) owner: &'static str,
    pub(crate) report: &'static str,
}

impl DecalSubmitter {
    pub(crate) fn prepare(decals: &[crate::pipeline_post::PostDecalGpu]) -> DecalSubmitPlan {
        let prepared = decals.iter().count();
        Self::upload_prepared_decal_report(prepared);
        DecalSubmitPlan {
            prepared_count: prepared,
            owner: "DecalSubmitter",
            report: "projected-decals-prepared",
        }
    }

    pub(crate) fn upload_prepared_decal_report(count: usize) {
        PREPARED_DECAL_REPORT.store(count, Ordering::Relaxed);
    }
}
