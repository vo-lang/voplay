mod encoder;
pub mod features;
mod portable_scene;
mod wire;

pub use encoder::*;
pub use features::*;
pub use portable_scene::*;
pub use voplay_runtime::presentation::FrameEncoderError;
pub use voplay_runtime::render_commands::{
    decode_frame_commands, decode_object_commands, encode_frame_commands, encode_object_commands,
    DrawCommand, PortableFrameEncoder, PortableFrameEncoderConfig, RetainedPortableFrameEncoder,
    PORTABLE_DRAW_COMPONENT,
};
pub use voplay_runtime::render_world::{
    Aabb3, RenderComponent, RenderComponentKind, RetainedRenderObject, TransformSample,
};
pub use voplay_runtime::sprite_animation::{
    SpriteAnimationConfig, SpriteAnimationError, SpriteAnimationFrame, SpriteAnimationSystem,
    SpriteAnimatorDescriptor, SpriteAnimatorId, SpriteClipDescriptor, SpriteCompletion,
    SpriteDrawPacket, SpriteFrameDescriptor, SpritePlaybackMode, SpritePulseRequest,
};
pub use wire::*;

pub const FEATURE_ABI: &str = "voplay.render2d/1";

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    FEATURE_ABI.as_ptr() as usize
}
