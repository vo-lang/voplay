use crate::model_loader::ModelManager;
use crate::primitive_pipeline::{PrimitiveDrawStats, PrimitivePipeline, PrimitiveRenderFilter};
use crate::primitive_scene::{PrimitiveChunkRef, PrimitiveDraw};
use crate::texture::TextureManager;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrimitiveSubmitReport {
    pub(crate) owner: &'static str,
    pub(crate) filter: PrimitiveRenderFilter,
    pub(crate) outcome: &'static str,
    pub(crate) stats: PrimitiveDrawStats,
}

impl PrimitiveSubmitter {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit<'a>(
        pipeline: &'a mut PrimitivePipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
        filter: PrimitiveRenderFilter,
    ) -> PrimitiveSubmitReport {
        let stats = pipeline.draw(
            device,
            queue,
            pass,
            draws,
            chunk_refs,
            models,
            textures,
            shadow_view,
            aux_targets_enabled,
            filter,
        );
        let outcome = classify_submit_outcome(stats);
        PrimitiveSubmitReport {
            stats,
            filter,
            owner: "PrimitiveSubmitter",
            outcome,
        }
    }

    pub(crate) fn filter_name(filter: PrimitiveRenderFilter) -> &'static str {
        match filter {
            crate::primitive_pipeline::PrimitiveRenderFilter::Main => "main",
            crate::primitive_pipeline::PrimitiveRenderFilter::Translucent => "translucent",
            crate::primitive_pipeline::PrimitiveRenderFilter::Water => "water",
        }
    }
}

fn classify_submit_outcome(stats: PrimitiveDrawStats) -> &'static str {
    if stats.batch_count > 0 {
        "submitted"
    } else if stats.skips.missing_resources() > 0 || stats.skips.invalid_batches() > 0 {
        "rejected"
    } else {
        "empty"
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_submit_outcome, PrimitiveDrawStats};

    #[test]
    fn submit_outcome_distinguishes_empty_submitted_and_rejected() {
        assert_eq!(
            classify_submit_outcome(PrimitiveDrawStats::default()),
            "empty"
        );
        assert_eq!(
            classify_submit_outcome(PrimitiveDrawStats {
                batch_count: 1,
                ..PrimitiveDrawStats::default()
            }),
            "submitted"
        );
        assert_eq!(
            classify_submit_outcome(PrimitiveDrawStats {
                skips: crate::render_world::RenderSkipStats {
                    missing_chunks: 1,
                    ..crate::render_world::RenderSkipStats::default()
                },
                ..PrimitiveDrawStats::default()
            }),
            "rejected"
        );
    }
}
