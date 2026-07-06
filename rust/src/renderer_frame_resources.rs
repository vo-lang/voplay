use super::renderer_frame::{RenderResource, RenderResourceKind};

pub(crate) const RES_SURFACE_COLOR: RenderResource = RenderResource {
    name: "surface-color",
    kind: RenderResourceKind::SurfaceColor,
};
pub(crate) const RES_MAIN_COLOR: RenderResource = RenderResource {
    name: "main-color",
    kind: RenderResourceKind::MainColor,
};
pub(crate) const RES_DEPTH: RenderResource = RenderResource {
    name: "depth",
    kind: RenderResourceKind::Depth,
};
pub(crate) const RES_RECEIVER_MASK: RenderResource = RenderResource {
    name: "receiver-mask",
    kind: RenderResourceKind::ReceiverMask,
};
pub(crate) const RES_SURFACE_PROPS: RenderResource = RenderResource {
    name: "surface-props",
    kind: RenderResourceKind::SurfaceProps,
};
pub(crate) const RES_SHADOW_MAP: RenderResource = RenderResource {
    name: "shadow-map",
    kind: RenderResourceKind::ShadowMap,
};
pub(crate) const RES_POST_COLOR: RenderResource = RenderResource {
    name: "post-color",
    kind: RenderResourceKind::PostColor,
};
pub(crate) const RES_WATER_COLOR: RenderResource = RenderResource {
    name: "water-color",
    kind: RenderResourceKind::WaterColor,
};
pub(crate) const RES_OVERLAY: RenderResource = RenderResource {
    name: "overlay",
    kind: RenderResourceKind::Overlay,
};
pub(crate) const RES_CAPTURE: RenderResource = RenderResource {
    name: "capture",
    kind: RenderResourceKind::Capture,
};
pub(crate) const RES_READBACK: RenderResource = RenderResource {
    name: "readback",
    kind: RenderResourceKind::Readback,
};
