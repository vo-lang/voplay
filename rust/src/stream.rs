//! Binary draw command stream decoder.
//! Reads opcodes + args from the []byte buffer produced by DrawCtx on the Vo side.

use crate::draw_protocol::{
    Opcode, DRAW_STREAM_FLAGS, DRAW_STREAM_HEADER_SIZE, DRAW_STREAM_MAGIC, DRAW_STREAM_VERSION,
};
use crate::math3d::{Quat, Vec3};
use crate::pipeline3d::MaterialOverride;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawStreamError {
    HeaderTooShort {
        actual: usize,
    },
    InvalidMagic {
        actual: [u8; 4],
    },
    UnsupportedVersion {
        actual: u16,
    },
    UnsupportedFlags {
        actual: u16,
    },
    PayloadLengthMismatch {
        declared: usize,
        actual: usize,
    },
    UnknownOpcode {
        offset: usize,
        opcode: u8,
    },
    Truncated {
        offset: usize,
        needed: usize,
        remaining: usize,
    },
    InvalidCount {
        offset: usize,
        count: usize,
        item_size: usize,
        remaining: usize,
    },
}

impl fmt::Display for DrawStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooShort { actual } => write!(
                formatter,
                "voplay: draw stream header is truncated: got {actual} bytes, need {DRAW_STREAM_HEADER_SIZE}"
            ),
            Self::InvalidMagic { actual } => write!(
                formatter,
                "voplay: draw stream magic mismatch: got {:02X?}, expected {:02X?}",
                actual, DRAW_STREAM_MAGIC
            ),
            Self::UnsupportedVersion { actual } => write!(
                formatter,
                "voplay: draw stream version {actual} is unsupported; expected {DRAW_STREAM_VERSION}"
            ),
            Self::UnsupportedFlags { actual } => {
                write!(formatter, "voplay: draw stream flags 0x{actual:04X} are unsupported")
            }
            Self::PayloadLengthMismatch { declared, actual } => write!(
                formatter,
                "voplay: draw stream payload length mismatch: header declares {declared} bytes, got {actual}"
            ),
            Self::UnknownOpcode { offset, opcode } => write!(
                formatter,
                "voplay: unknown draw stream opcode 0x{opcode:02X} at byte {offset}"
            ),
            Self::Truncated {
                offset,
                needed,
                remaining,
            } => write!(
                formatter,
                "voplay: draw stream truncated at byte {offset}: need {needed} bytes, have {remaining}"
            ),
            Self::InvalidCount {
                offset,
                count,
                item_size,
                remaining,
            } => write!(
                formatter,
                "voplay: draw stream count {count} at byte {offset} requires {item_size} bytes per item with {remaining} bytes remaining"
            ),
        }
    }
}

impl std::error::Error for DrawStreamError {}

/// Decoded light from SetLights3D command.
#[derive(Debug, Clone)]
pub struct DecodedLight {
    pub light_type: u8,  // 0 = directional, 1 = point
    pub position: Vec3,  // position (point) or unused (dir)
    pub direction: Vec3, // direction (dir) or unused (point)
    pub color: Vec3,     // RGB color
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct Primitive3DInstanceCommand {
    pub object_id: u32,
    pub model_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub material: MaterialOverride,
    pub visible: bool,
    pub flags: u32,
    pub lod_near: f32,
    pub lod_far: f32,
    pub wind_strength: f32,
    pub atlas_uv: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Primitive3DInstanceRefCommand {
    pub object_id: u32,
    pub model_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub material_id: u32,
    pub visible: bool,
    pub flags: u32,
    pub lod_near: f32,
    pub lod_far: f32,
    pub wind_strength: f32,
    pub atlas_uv: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Primitive3DInstanceKeyCommand {
    pub object_id: u32,
    pub shape_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub material_id: u32,
    pub tint: [f32; 4],
    pub visible: bool,
    pub flags: u32,
    pub lod_near: f32,
    pub lod_far: f32,
    pub wind_strength: f32,
    pub atlas_uv: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Primitive3DMaterialCommand {
    pub material_id: u32,
    pub material: MaterialOverride,
}

#[derive(Debug, Clone)]
pub struct Primitive3DShapeCommand {
    pub shape_id: u32,
    pub model_id: u32,
}

/// Decoded draw command.
#[derive(Debug)]
pub enum DrawCommand {
    Clear {
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
    SetCamera2D {
        x: f32,
        y: f32,
        zoom: f32,
        rotation: f32,
    },
    ResetCamera,
    SetLayer {
        z: u16,
    },
    DrawRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
    DrawCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
    DrawLine {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
    DrawText {
        x: f32,
        y: f32,
        size: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
        text: String,
    },
    SetFont {
        font_id: u32,
    },
    SetCamera3D {
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        fov: f32,
        near: f32,
        far: f32,
    },
    SetLights3D {
        ambient_r: f32,
        ambient_g: f32,
        ambient_b: f32,
        ambient_ground_r: f32,
        ambient_ground_g: f32,
        ambient_ground_b: f32,
        lights: Vec<DecodedLight>,
    },
    SetFog3D {
        mode: u8,
        color: Vec3,
        start: f32,
        end: f32,
        density: f32,
    },
    SetColorGrading3D {
        tone_map: u8,
        exposure: f32,
        contrast: f32,
        saturation: f32,
    },
    SetShadow3D {
        enabled: bool,
        resolution: u32,
        strength: f32,
        softness: f32,
        distance: f32,
        fade: f32,
        quality: u32,
    },
    SetRenderDebug3D {
        mode: u8,
    },
    SetPostProcess3D {
        bloom_threshold: f32,
        bloom_strength: f32,
        sharpen_strength: f32,
        fxaa_strength: f32,
    },
    SetContactAO3D {
        strength: f32,
        radius: f32,
        depth_scale: f32,
        detail_strength: f32,
        detail_radius: f32,
        normal_bias: f32,
        quality: u32,
    },
    DrawSkybox {
        cubemap_id: u32,
    },
    DrawProjectedDecal3D {
        position: Vec3,
        yaw: f32,
        width: f32,
        length: f32,
        depth: f32,
        color: [f32; 4],
    },
    SetProjectedDecalAtlas3D {
        atlas_id: u32,
    },
    SetProjectedDecalNormalAtlas3D {
        atlas_id: u32,
    },
    SetProjectedDecalRoughnessAtlas3D {
        atlas_id: u32,
    },
    SetProjectedDecalMaskAtlas3D {
        atlas_id: u32,
    },
    SetProjectedDecalDistanceFade3D {
        start: f32,
        end: f32,
    },
    SetProjectedDecalAngleFade3D {
        start: f32,
        end: f32,
    },
    SetProjectedDecalReceiverMask3D {
        mask: u32,
    },
    SetProjectedDecalSurfaceResponse3D {
        normal_strength: f32,
        roughness: f32,
        roughness_strength: f32,
    },
    DrawProjectedDecal3DUV {
        position: Vec3,
        yaw: f32,
        width: f32,
        length: f32,
        depth: f32,
        color: [f32; 4],
        uv_rect: [f32; 4],
    },
    DrawModel {
        model_id: u32,
        pos: Vec3,
        rot: Quat,
        scale: Vec3,
        material: MaterialOverride,
        animation_world_id: u32,
        animation_target_id: u32,
    },
    Scene3DUpsertObject {
        scene_id: u32,
        object_id: u32,
        model_id: u32,
        pos: Vec3,
        rot: Quat,
        scale: Vec3,
        material: MaterialOverride,
        visible: bool,
        animation_world_id: u32,
        animation_target_id: u32,
    },
    Scene3DDestroyObject {
        scene_id: u32,
        object_id: u32,
    },
    Scene3DClear {
        scene_id: u32,
    },
    Scene3DDraw {
        scene_id: u32,
    },
    Primitive3DUpsertInstance {
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
        model_id: u32,
        pos: Vec3,
        rot: Quat,
        scale: Vec3,
        material: MaterialOverride,
        visible: bool,
        flags: u32,
        lod_near: f32,
        lod_far: f32,
        wind_strength: f32,
        atlas_uv: [f32; 4],
    },
    Primitive3DDestroyInstance {
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
    },
    Primitive3DClearLayer {
        scene_id: u32,
        layer_id: u32,
    },
    Primitive3DDestroyLayer {
        scene_id: u32,
        layer_id: u32,
    },
    Primitive3DReplaceChunk {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceCommand>,
    },
    Primitive3DReplaceChunkRefs {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceRefCommand>,
    },
    Primitive3DUpsertMaterials {
        scene_id: u32,
        layer_id: u32,
        materials: Vec<Primitive3DMaterialCommand>,
    },
    Primitive3DUpsertShapes {
        scene_id: u32,
        layer_id: u32,
        shapes: Vec<Primitive3DShapeCommand>,
    },
    Primitive3DReplaceChunkKeys {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceKeyCommand>,
    },
    Primitive3DSetChunkVisible {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        visible: bool,
    },
    DrawSprite {
        tex_id: u32,
        src_x: f32,
        src_y: f32,
        src_w: f32,
        src_h: f32,
        dst_x: f32,
        dst_y: f32,
        dst_w: f32,
        dst_h: f32,
        flip_x: bool,
        flip_y: bool,
        rotation: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    },
    DrawBillboard {
        tex_id: u32,
        src_x: f32,
        src_y: f32,
        src_w: f32,
        src_h: f32,
        world_pos: Vec3,
        w: f32,
        h: f32,
        tint: [f32; 4],
    },
}

mod reader;
pub use reader::StreamReader;

#[cfg(test)]
mod tests;
