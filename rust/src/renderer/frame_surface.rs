use super::*;

fn surface_failure(code: &str, action: &str, recoverable: bool, detail: &str) -> String {
    format!(
        "voplay.render.failure stage=surface_acquire code={code} action={action} recoverable={recoverable} detail={detail:?}"
    )
}

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
                    surface_failure("recovery_failed", "abort_frame", true, &error.to_string())
                })?
            }
            Err(wgpu::SurfaceError::Timeout) => {
                return Err(surface_failure(
                    "timeout",
                    "skip_frame",
                    true,
                    "acquire timed out",
                ));
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err(surface_failure(
                    "out_of_memory",
                    "abort_renderer",
                    false,
                    "surface allocation failed",
                ));
            }
            Err(error) => {
                return Err(surface_failure(
                    "acquire_failed",
                    "abort_frame",
                    true,
                    &error.to_string(),
                ));
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok((output, view, elapsed_ms_opt(acquire_start)))
    }
}

#[cfg(test)]
mod tests {
    use super::surface_failure;

    #[test]
    fn surface_failure_is_machine_attributable() {
        let failure = surface_failure("timeout", "skip_frame", true, "acquire timed out");
        assert!(failure.contains("stage=surface_acquire"));
        assert!(failure.contains("code=timeout"));
        assert!(failure.contains("action=skip_frame"));
        assert!(failure.contains("recoverable=true"));
    }
}
