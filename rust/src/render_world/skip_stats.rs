#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderSkipStats {
    pub filtered_draws: u32,
    pub missing_models: u32,
    pub missing_meshes: u32,
    pub missing_textures: u32,
    pub missing_bind_groups: u32,
    pub missing_chunks: u32,
    pub missing_targets: u32,
    pub invalid_batch_indices: u32,
    pub incompatible_draws: u32,
    pub fallback_paths: u32,
}

impl RenderSkipStats {
    pub fn missing_resources(self) -> u32 {
        self.missing_models
            .saturating_add(self.missing_meshes)
            .saturating_add(self.missing_textures)
            .saturating_add(self.missing_bind_groups)
            .saturating_add(self.missing_chunks)
            .saturating_add(self.missing_targets)
    }

    pub fn invalid_batches(self) -> u32 {
        self.invalid_batch_indices
            .saturating_add(self.incompatible_draws)
    }

    pub fn merge(&mut self, other: Self) {
        self.filtered_draws = self.filtered_draws.saturating_add(other.filtered_draws);
        self.missing_models = self.missing_models.saturating_add(other.missing_models);
        self.missing_meshes = self.missing_meshes.saturating_add(other.missing_meshes);
        self.missing_textures = self.missing_textures.saturating_add(other.missing_textures);
        self.missing_bind_groups = self
            .missing_bind_groups
            .saturating_add(other.missing_bind_groups);
        self.missing_chunks = self.missing_chunks.saturating_add(other.missing_chunks);
        self.missing_targets = self.missing_targets.saturating_add(other.missing_targets);
        self.invalid_batch_indices = self
            .invalid_batch_indices
            .saturating_add(other.invalid_batch_indices);
        self.incompatible_draws = self
            .incompatible_draws
            .saturating_add(other.incompatible_draws);
        self.fallback_paths = self.fallback_paths.saturating_add(other.fallback_paths);
    }
}
