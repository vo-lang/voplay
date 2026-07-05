use super::*;

impl Renderer {
    pub(crate) fn acquire_surface_texture(
        &mut self,
        perf_enabled: bool,
    ) -> Result<(wgpu::SurfaceTexture, wgpu::TextureView, f64), String> {
        let acquire_start = if perf_enabled { Some(perf_now()) } else { None };
        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.surface.get_current_texture().map_err(|error| {
                    format!(
                        "voplay: get_current_texture recovery after lost/outdated failed: {error}"
                    )
                })?
            }
            Err(wgpu::SurfaceError::Timeout) => {
                return Err(
                    "voplay: get_current_texture timeout; frame skipped before submit".to_string(),
                );
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err(
                    "voplay: get_current_texture out of memory; renderer cannot recover"
                        .to_string(),
                );
            }
            Err(error) => return Err(format!("voplay: get_current_texture failed: {error}")),
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok((output, view, elapsed_ms_opt(acquire_start)))
    }
}
