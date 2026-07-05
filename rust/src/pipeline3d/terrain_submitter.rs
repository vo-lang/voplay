#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerrainSubmitter;

impl TerrainSubmitter {
    pub(crate) fn prepare(draws: &[super::ModelDraw], models: &crate::model_loader::ModelManager) -> u32 {
        let mut count = 0u32;
        for draw in draws {
            let Some(model) = models.get(draw.model_id) else {
                continue;
            };
            for mesh in &model.meshes {
                if !mesh.skinned && mesh.material.control_texture_id.is_some() {
                    count = count.saturating_add(1);
                }
            }
        }
        count
    }
}
