#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitter;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshSubmitProfile {
    pub(crate) instance_count: u32,
    pub(crate) batch_hint: usize,
}

impl MeshSubmitter {
    pub(crate) fn prepare(draws: &[super::ModelDraw], models: &crate::model_loader::ModelManager) -> MeshSubmitProfile {
        let mut instance_count = 0u32;
        let mut batch_hint = 0usize;
        for draw in draws {
            let Some(model) = models.get(draw.model_id) else {
                continue;
            };
            for mesh in &model.meshes {
                if mesh.skinned || mesh.material.control_texture_id.is_some() {
                    continue;
                }
                instance_count = instance_count.saturating_add(1);
                batch_hint = batch_hint.saturating_add(1);
            }
        }
        MeshSubmitProfile { instance_count, batch_hint }
    }
}
