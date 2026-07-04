pub(crate) const MAIN_SAMPLE_COUNT: u32 = 1;
pub(crate) const MAIN_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub(crate) const RECEIVER_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(crate) const SURFACE_PROPS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(crate) fn create_depth_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
) -> wgpu::TextureView {
    let usage = if sample_count > 1 {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    };
    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay_depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: MAIN_DEPTH_FORMAT,
        usage,
        view_formats: &[],
    });
    depth_texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_msaa_color_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> Option<wgpu::TextureView> {
    if sample_count <= 1 {
        return None;
    }
    let color_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay_main_msaa_color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    Some(color_texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

pub(crate) fn create_post_color_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    let color_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay_post_color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    color_texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_receiver_mask_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: RECEIVER_MASK_FORMAT,
        usage,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_msaa_receiver_mask_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
) -> Option<wgpu::TextureView> {
    if sample_count <= 1 {
        return None;
    }
    Some(create_receiver_mask_view(
        device,
        width,
        height,
        sample_count,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
        "voplay_receiver_mask_msaa",
    ))
}

pub(crate) fn create_surface_props_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: SURFACE_PROPS_FORMAT,
        usage,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_msaa_surface_props_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
) -> Option<wgpu::TextureView> {
    if sample_count <= 1 {
        return None;
    }
    Some(create_surface_props_view(
        device,
        width,
        height,
        sample_count,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
        "voplay_surface_props_msaa",
    ))
}

#[derive(Default)]
pub(crate) struct RendererTargetRegistry {
    pub(crate) depth_view: Option<wgpu::TextureView>,
    pub(crate) msaa_color_view: Option<wgpu::TextureView>,
    pub(crate) post_color_view: Option<wgpu::TextureView>,
    pub(crate) msaa_receiver_mask_view: Option<wgpu::TextureView>,
    pub(crate) receiver_mask_view: Option<wgpu::TextureView>,
    pub(crate) msaa_surface_props_view: Option<wgpu::TextureView>,
    pub(crate) surface_props_view: Option<wgpu::TextureView>,
}
