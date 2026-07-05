#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MaterialBinder;

impl MaterialBinder {
    pub(crate) fn bind_model_group<'a>(
        render_pass: &mut wgpu::RenderPass<'a>,
        bind_group: &'a wgpu::BindGroup,
        offset: u32,
    ) {
        render_pass.set_bind_group(1, bind_group, &[offset]);
    }
}
