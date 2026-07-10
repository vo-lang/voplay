#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecalSubmitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecalSubmitPlan {
    pub(crate) rejected_count: usize,
    pub(crate) upload_bytes: u32,
}

fn decal_submit_counts(count: usize) -> (usize, usize) {
    let prepared = count.min(crate::pipeline_post::MAX_POST_DECALS);
    (prepared, count.saturating_sub(prepared))
}

impl DecalSubmitter {
    pub(crate) fn prepare_and_upload(
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        inv_view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
        decals: &[crate::pipeline_post::PostDecalGpu],
        atlas_count: u32,
    ) -> DecalSubmitPlan {
        let (prepared, rejected) = decal_submit_counts(decals.len());
        let uniform = crate::pipeline_post::PostDecalUniform::from_decals(
            inv_view_proj,
            camera_pos,
            &decals[..prepared],
            atlas_count,
        );
        let bytes = bytemuck::bytes_of(&uniform);
        queue.write_buffer(buffer, 0, bytes);
        DecalSubmitPlan {
            rejected_count: rejected,
            upload_bytes: bytes.len().min(u32::MAX as usize) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decal_submitter_reports_capacity_rejections() {
        assert_eq!(decal_submit_counts(0), (0, 0));
        assert_eq!(
            decal_submit_counts(crate::pipeline_post::MAX_POST_DECALS + 3),
            (crate::pipeline_post::MAX_POST_DECALS, 3)
        );
    }
}
