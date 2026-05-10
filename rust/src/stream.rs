//! Binary draw command stream decoder.
//! Reads opcodes + args from the []byte buffer produced by DrawCtx on the Vo side.

use crate::math3d::{Quat, Vec3};
use crate::pipeline3d::MaterialOverride;

/// Draw command opcodes (must match draw.vo constants).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Clear = 0x01,
    SetCamera2D = 0x02,
    SetCamera3D = 0x03,
    ResetCamera = 0x04,
    SetLayer = 0x05,
    SetFont = 0x06,
    DrawSprite = 0x10,
    DrawRect = 0x11,
    DrawCircle = 0x12,
    DrawLine = 0x13,
    DrawText = 0x14,
    DrawModel = 0x20,
    DrawBillboard = 0x21,
    DrawProjectedDecal3D = 0x22,
    SetLights3D = 0x23,
    SetFog3D = 0x24,
    DrawSkybox = 0x25,
    SetColorGrading3D = 0x26,
    SetShadow3D = 0x27,
    SetRenderDebug3D = 0x28,
    SetPostProcess3D = 0x29,
    SetContactAO3D = 0x2A,
    SetProjectedDecalAtlas3D = 0x2B,
    DrawProjectedDecal3DUV = 0x2C,
    SetProjectedDecalDistanceFade3D = 0x2D,
    SetProjectedDecalReceiverMask3D = 0x2E,
    SetProjectedDecalSurfaceResponse3D = 0x2F,
    Scene3DUpsertObject = 0x30,
    Scene3DDestroyObject = 0x31,
    Scene3DClear = 0x32,
    Scene3DDraw = 0x33,
    Primitive3DUpsertInstance = 0x34,
    Primitive3DDestroyInstance = 0x35,
    Primitive3DClearLayer = 0x36,
    Primitive3DDestroyLayer = 0x37,
    Primitive3DReplaceChunk = 0x38,
    Primitive3DReplaceChunkRefs = 0x39,
    Primitive3DUpsertMaterials = 0x3A,
    Primitive3DUpsertShapes = 0x3B,
    Primitive3DReplaceChunkKeys = 0x3C,
    Primitive3DSetChunkVisible = 0x3D,
    SetProjectedDecalNormalAtlas3D = 0x40,
    SetProjectedDecalRoughnessAtlas3D = 0x41,
    SetProjectedDecalMaskAtlas3D = 0x42,
    SetProjectedDecalAngleFade3D = 0x43,
}

impl Opcode {
    pub fn from_u8(v: u8) -> Option<Opcode> {
        match v {
            0x01 => Some(Opcode::Clear),
            0x02 => Some(Opcode::SetCamera2D),
            0x03 => Some(Opcode::SetCamera3D),
            0x04 => Some(Opcode::ResetCamera),
            0x05 => Some(Opcode::SetLayer),
            0x06 => Some(Opcode::SetFont),
            0x10 => Some(Opcode::DrawSprite),
            0x11 => Some(Opcode::DrawRect),
            0x12 => Some(Opcode::DrawCircle),
            0x13 => Some(Opcode::DrawLine),
            0x14 => Some(Opcode::DrawText),
            0x20 => Some(Opcode::DrawModel),
            0x21 => Some(Opcode::DrawBillboard),
            0x22 => Some(Opcode::DrawProjectedDecal3D),
            0x23 => Some(Opcode::SetLights3D),
            0x24 => Some(Opcode::SetFog3D),
            0x25 => Some(Opcode::DrawSkybox),
            0x26 => Some(Opcode::SetColorGrading3D),
            0x27 => Some(Opcode::SetShadow3D),
            0x28 => Some(Opcode::SetRenderDebug3D),
            0x29 => Some(Opcode::SetPostProcess3D),
            0x2A => Some(Opcode::SetContactAO3D),
            0x2B => Some(Opcode::SetProjectedDecalAtlas3D),
            0x2C => Some(Opcode::DrawProjectedDecal3DUV),
            0x2D => Some(Opcode::SetProjectedDecalDistanceFade3D),
            0x2E => Some(Opcode::SetProjectedDecalReceiverMask3D),
            0x2F => Some(Opcode::SetProjectedDecalSurfaceResponse3D),
            0x30 => Some(Opcode::Scene3DUpsertObject),
            0x31 => Some(Opcode::Scene3DDestroyObject),
            0x32 => Some(Opcode::Scene3DClear),
            0x33 => Some(Opcode::Scene3DDraw),
            0x34 => Some(Opcode::Primitive3DUpsertInstance),
            0x35 => Some(Opcode::Primitive3DDestroyInstance),
            0x36 => Some(Opcode::Primitive3DClearLayer),
            0x37 => Some(Opcode::Primitive3DDestroyLayer),
            0x38 => Some(Opcode::Primitive3DReplaceChunk),
            0x39 => Some(Opcode::Primitive3DReplaceChunkRefs),
            0x3A => Some(Opcode::Primitive3DUpsertMaterials),
            0x3B => Some(Opcode::Primitive3DUpsertShapes),
            0x3C => Some(Opcode::Primitive3DReplaceChunkKeys),
            0x3D => Some(Opcode::Primitive3DSetChunkVisible),
            0x40 => Some(Opcode::SetProjectedDecalNormalAtlas3D),
            0x41 => Some(Opcode::SetProjectedDecalRoughnessAtlas3D),
            0x42 => Some(Opcode::SetProjectedDecalMaskAtlas3D),
            0x43 => Some(Opcode::SetProjectedDecalAngleFade3D),
            _ => None,
        }
    }
}

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
#[allow(dead_code)]
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

/// Stream reader for binary draw commands.
pub struct StreamReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> StreamReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_material(&mut self) -> MaterialOverride {
        MaterialOverride {
            id: self.read_u32(),
            base_color: [
                self.read_f32(),
                self.read_f32(),
                self.read_f32(),
                self.read_f32(),
            ],
            albedo_texture_id: self.read_u32(),
            normal_texture_id: self.read_u32(),
            metallic_roughness_texture_id: self.read_u32(),
            emissive_texture_id: self.read_u32(),
            mask_texture_id: self.read_u32(),
            emissive_color: [
                self.read_f32(),
                self.read_f32(),
                self.read_f32(),
                self.read_f32(),
            ],
            roughness: self.read_f32(),
            metallic: self.read_f32(),
            normal_scale: self.read_f32(),
            uv_scale: self.read_f32(),
            toon_ramp_texture_id: self.read_u32(),
            detail_strength: self.read_f32(),
            macro_blend: self.read_f32(),
            roughness_response: self.read_f32(),
            toon_ramp_response: self.read_f32(),
            shading_mode: self.read_u32(),
            wrap_mode: self.read_u32(),
            filter_mode: self.read_u32(),
        }
    }

    fn read_primitive3d_instance(&mut self) -> Primitive3DInstanceCommand {
        let object_id = self.read_u32();
        let model_id = self.read_u32();
        let pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let rot = Quat::new(
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        );
        let scale = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let material = self.read_material();
        let visible = self.read_u8() != 0;
        let flags = self.read_u32();
        let lod_near = self.read_f32();
        let lod_far = self.read_f32();
        let wind_strength = self.read_f32();
        let atlas_uv = [
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        ];
        Primitive3DInstanceCommand {
            object_id,
            model_id,
            pos,
            rot,
            scale,
            material,
            visible,
            flags,
            lod_near,
            lod_far,
            wind_strength,
            atlas_uv,
        }
    }

    fn read_primitive3d_instance_ref(&mut self) -> Primitive3DInstanceRefCommand {
        let object_id = self.read_u32();
        let model_id = self.read_u32();
        let pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let rot = Quat::new(
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        );
        let scale = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let material_id = self.read_u32();
        let visible = self.read_u8() != 0;
        let flags = self.read_u32();
        let lod_near = self.read_f32();
        let lod_far = self.read_f32();
        let wind_strength = self.read_f32();
        let atlas_uv = [
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        ];
        Primitive3DInstanceRefCommand {
            object_id,
            model_id,
            pos,
            rot,
            scale,
            material_id,
            visible,
            flags,
            lod_near,
            lod_far,
            wind_strength,
            atlas_uv,
        }
    }

    fn read_primitive3d_instance_key(&mut self) -> Primitive3DInstanceKeyCommand {
        let object_id = self.read_u32();
        let shape_id = self.read_u32();
        let pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let rot = Quat::new(
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        );
        let scale = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
        let material_id = self.read_u32();
        let tint = [
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        ];
        let visible = self.read_u8() != 0;
        let flags = self.read_u32();
        let lod_near = self.read_f32();
        let lod_far = self.read_f32();
        let wind_strength = self.read_f32();
        let atlas_uv = [
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
            self.read_f32(),
        ];
        Primitive3DInstanceKeyCommand {
            object_id,
            shape_id,
            pos,
            rot,
            scale,
            material_id,
            tint,
            visible,
            flags,
            lod_near,
            lod_far,
            wind_strength,
            atlas_uv,
        }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn check_remaining(&self, n: usize) {
        assert!(
            self.remaining() >= n,
            "voplay: draw stream truncated at pos {} (need {} bytes, have {})",
            self.pos,
            n,
            self.remaining()
        );
    }

    fn read_u8(&mut self) -> u8 {
        self.check_remaining(1);
        let v = self.data[self.pos];
        self.pos += 1;
        v
    }

    fn read_u16(&mut self) -> u16 {
        self.check_remaining(2);
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        v
    }

    fn read_u32(&mut self) -> u32 {
        self.check_remaining(4);
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }

    fn read_f32(&mut self) -> f32 {
        self.check_remaining(4);
        let bytes = [
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ];
        self.pos += 4;
        f32::from_le_bytes(bytes)
    }

    /// Decode the next command from the stream. Returns None when stream is exhausted.
    pub fn next_command(&mut self) -> Option<DrawCommand> {
        if self.remaining() == 0 {
            return None;
        }

        let op_byte = self.read_u8();
        let op = Opcode::from_u8(op_byte)?;

        match op {
            Opcode::Clear => {
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::Clear { r, g, b, a })
            }
            Opcode::SetCamera2D => {
                let x = self.read_f32();
                let y = self.read_f32();
                let zoom = self.read_f32();
                let rotation = self.read_f32();
                Some(DrawCommand::SetCamera2D {
                    x,
                    y,
                    zoom,
                    rotation,
                })
            }
            Opcode::ResetCamera => Some(DrawCommand::ResetCamera),
            Opcode::SetLayer => {
                let z = self.read_u16();
                Some(DrawCommand::SetLayer { z })
            }
            Opcode::SetFont => {
                let font_id = self.read_u32();
                Some(DrawCommand::SetFont { font_id })
            }
            Opcode::DrawRect => {
                let x = self.read_f32();
                let y = self.read_f32();
                let w = self.read_f32();
                let h = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::DrawRect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                    a,
                })
            }
            Opcode::DrawCircle => {
                let cx = self.read_f32();
                let cy = self.read_f32();
                let radius = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::DrawCircle {
                    cx,
                    cy,
                    radius,
                    r,
                    g,
                    b,
                    a,
                })
            }
            Opcode::DrawLine => {
                let x1 = self.read_f32();
                let y1 = self.read_f32();
                let x2 = self.read_f32();
                let y2 = self.read_f32();
                let thickness = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::DrawLine {
                    x1,
                    y1,
                    x2,
                    y2,
                    thickness,
                    r,
                    g,
                    b,
                    a,
                })
            }
            Opcode::DrawText => {
                let x = self.read_f32();
                let y = self.read_f32();
                let size = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                let len = self.read_u16() as usize;
                self.check_remaining(len);
                let text =
                    String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
                self.pos += len;
                Some(DrawCommand::DrawText {
                    x,
                    y,
                    size,
                    r,
                    g,
                    b,
                    a,
                    text,
                })
            }
            Opcode::DrawSprite => {
                let tex_id = self.read_u32();
                let src_x = self.read_f32();
                let src_y = self.read_f32();
                let src_w = self.read_f32();
                let src_h = self.read_f32();
                let dst_x = self.read_f32();
                let dst_y = self.read_f32();
                let dst_w = self.read_f32();
                let dst_h = self.read_f32();
                let flip_x = self.read_u8() != 0;
                let flip_y = self.read_u8() != 0;
                let rotation = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::DrawSprite {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    dst_x,
                    dst_y,
                    dst_w,
                    dst_h,
                    flip_x,
                    flip_y,
                    rotation,
                    r,
                    g,
                    b,
                    a,
                })
            }
            Opcode::SetCamera3D => {
                let eye = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let target = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let up = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let fov = self.read_f32();
                let near = self.read_f32();
                let far = self.read_f32();
                Some(DrawCommand::SetCamera3D {
                    eye,
                    target,
                    up,
                    fov,
                    near,
                    far,
                })
            }
            Opcode::SetLights3D => {
                let ambient_r = self.read_f32();
                let ambient_g = self.read_f32();
                let ambient_b = self.read_f32();
                let ambient_ground_r = self.read_f32();
                let ambient_ground_g = self.read_f32();
                let ambient_ground_b = self.read_f32();
                let count = self.read_u8() as usize;
                let mut lights = Vec::with_capacity(count);
                for _ in 0..count {
                    let light_type = self.read_u8();
                    let position = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                    let direction = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                    let color = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                    let intensity = self.read_f32();
                    lights.push(DecodedLight {
                        light_type,
                        position,
                        direction,
                        color,
                        intensity,
                    });
                }
                Some(DrawCommand::SetLights3D {
                    ambient_r,
                    ambient_g,
                    ambient_b,
                    ambient_ground_r,
                    ambient_ground_g,
                    ambient_ground_b,
                    lights,
                })
            }
            Opcode::SetFog3D => {
                let mode = self.read_u8();
                let color = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let start = self.read_f32();
                let end = self.read_f32();
                let density = self.read_f32();
                Some(DrawCommand::SetFog3D {
                    mode,
                    color,
                    start,
                    end,
                    density,
                })
            }
            Opcode::SetColorGrading3D => {
                let tone_map = self.read_u8();
                let exposure = self.read_f32();
                let contrast = self.read_f32();
                let saturation = self.read_f32();
                Some(DrawCommand::SetColorGrading3D {
                    tone_map,
                    exposure,
                    contrast,
                    saturation,
                })
            }
            Opcode::SetShadow3D => {
                let enabled = self.read_u8() != 0;
                let resolution = self.read_u32();
                let strength = self.read_f32();
                let softness = self.read_f32();
                let distance = self.read_f32();
                let fade = self.read_f32();
                let quality = self.read_u32();
                Some(DrawCommand::SetShadow3D {
                    enabled,
                    resolution,
                    strength,
                    softness,
                    distance,
                    fade,
                    quality,
                })
            }
            Opcode::SetRenderDebug3D => {
                let mode = self.read_u8();
                Some(DrawCommand::SetRenderDebug3D { mode })
            }
            Opcode::SetPostProcess3D => {
                let bloom_threshold = self.read_f32();
                let bloom_strength = self.read_f32();
                let sharpen_strength = self.read_f32();
                let fxaa_strength = self.read_f32();
                Some(DrawCommand::SetPostProcess3D {
                    bloom_threshold,
                    bloom_strength,
                    sharpen_strength,
                    fxaa_strength,
                })
            }
            Opcode::SetContactAO3D => {
                let strength = self.read_f32();
                let radius = self.read_f32();
                let depth_scale = self.read_f32();
                let detail_strength = self.read_f32();
                let detail_radius = self.read_f32();
                let normal_bias = self.read_f32();
                let quality = self.read_u32();
                Some(DrawCommand::SetContactAO3D {
                    strength,
                    radius,
                    depth_scale,
                    detail_strength,
                    detail_radius,
                    normal_bias,
                    quality,
                })
            }
            Opcode::DrawSkybox => {
                let cubemap_id = self.read_u32();
                Some(DrawCommand::DrawSkybox { cubemap_id })
            }
            Opcode::DrawProjectedDecal3D => {
                let position = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let yaw = self.read_f32();
                let width = self.read_f32();
                let length = self.read_f32();
                let depth = self.read_f32();
                let color = [
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                ];
                Some(DrawCommand::DrawProjectedDecal3D {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                })
            }
            Opcode::SetProjectedDecalAtlas3D => {
                let atlas_id = self.read_u32();
                Some(DrawCommand::SetProjectedDecalAtlas3D { atlas_id })
            }
            Opcode::SetProjectedDecalNormalAtlas3D => {
                let atlas_id = self.read_u32();
                Some(DrawCommand::SetProjectedDecalNormalAtlas3D { atlas_id })
            }
            Opcode::SetProjectedDecalRoughnessAtlas3D => {
                let atlas_id = self.read_u32();
                Some(DrawCommand::SetProjectedDecalRoughnessAtlas3D { atlas_id })
            }
            Opcode::SetProjectedDecalMaskAtlas3D => {
                let atlas_id = self.read_u32();
                Some(DrawCommand::SetProjectedDecalMaskAtlas3D { atlas_id })
            }
            Opcode::SetProjectedDecalDistanceFade3D => {
                let start = self.read_f32();
                let end = self.read_f32();
                Some(DrawCommand::SetProjectedDecalDistanceFade3D { start, end })
            }
            Opcode::SetProjectedDecalAngleFade3D => {
                let start = self.read_f32();
                let end = self.read_f32();
                Some(DrawCommand::SetProjectedDecalAngleFade3D { start, end })
            }
            Opcode::SetProjectedDecalReceiverMask3D => {
                let mask = self.read_u32();
                Some(DrawCommand::SetProjectedDecalReceiverMask3D { mask })
            }
            Opcode::SetProjectedDecalSurfaceResponse3D => {
                let normal_strength = self.read_f32();
                let roughness = self.read_f32();
                let roughness_strength = self.read_f32();
                Some(DrawCommand::SetProjectedDecalSurfaceResponse3D {
                    normal_strength,
                    roughness,
                    roughness_strength,
                })
            }
            Opcode::DrawProjectedDecal3DUV => {
                let position = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let yaw = self.read_f32();
                let width = self.read_f32();
                let length = self.read_f32();
                let depth = self.read_f32();
                let color = [
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                ];
                let uv_rect = [
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                ];
                Some(DrawCommand::DrawProjectedDecal3DUV {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                    uv_rect,
                })
            }
            Opcode::DrawModel => {
                let model_id = self.read_u32();
                let pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let rot = Quat::new(
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                );
                let scale = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let material = self.read_material();
                let animation_world_id = self.read_u32();
                let animation_target_id = self.read_u32();
                Some(DrawCommand::DrawModel {
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    animation_world_id,
                    animation_target_id,
                })
            }
            Opcode::Scene3DUpsertObject => {
                let scene_id = self.read_u32();
                let object_id = self.read_u32();
                let model_id = self.read_u32();
                let pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let rot = Quat::new(
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                );
                let scale = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let material = self.read_material();
                let visible = self.read_u8() != 0;
                let animation_world_id = self.read_u32();
                let animation_target_id = self.read_u32();
                Some(DrawCommand::Scene3DUpsertObject {
                    scene_id,
                    object_id,
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    visible,
                    animation_world_id,
                    animation_target_id,
                })
            }
            Opcode::Scene3DDestroyObject => {
                let scene_id = self.read_u32();
                let object_id = self.read_u32();
                Some(DrawCommand::Scene3DDestroyObject {
                    scene_id,
                    object_id,
                })
            }
            Opcode::Scene3DClear => {
                let scene_id = self.read_u32();
                Some(DrawCommand::Scene3DClear { scene_id })
            }
            Opcode::Scene3DDraw => {
                let scene_id = self.read_u32();
                Some(DrawCommand::Scene3DDraw { scene_id })
            }
            Opcode::Primitive3DUpsertInstance => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let instance = self.read_primitive3d_instance();
                Some(DrawCommand::Primitive3DUpsertInstance {
                    scene_id,
                    layer_id,
                    object_id: instance.object_id,
                    model_id: instance.model_id,
                    pos: instance.pos,
                    rot: instance.rot,
                    scale: instance.scale,
                    material: instance.material,
                    visible: instance.visible,
                    flags: instance.flags,
                    lod_near: instance.lod_near,
                    lod_far: instance.lod_far,
                    wind_strength: instance.wind_strength,
                    atlas_uv: instance.atlas_uv,
                })
            }
            Opcode::Primitive3DDestroyInstance => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let object_id = self.read_u32();
                Some(DrawCommand::Primitive3DDestroyInstance {
                    scene_id,
                    layer_id,
                    object_id,
                })
            }
            Opcode::Primitive3DClearLayer => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                Some(DrawCommand::Primitive3DClearLayer { scene_id, layer_id })
            }
            Opcode::Primitive3DDestroyLayer => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                Some(DrawCommand::Primitive3DDestroyLayer { scene_id, layer_id })
            }
            Opcode::Primitive3DReplaceChunk => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let chunk_id = self.read_u32();
                let count = self.read_u32();
                let mut instances = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    instances.push(self.read_primitive3d_instance());
                }
                Some(DrawCommand::Primitive3DReplaceChunk {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                })
            }
            Opcode::Primitive3DReplaceChunkRefs => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let chunk_id = self.read_u32();
                let count = self.read_u32();
                let mut instances = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    instances.push(self.read_primitive3d_instance_ref());
                }
                Some(DrawCommand::Primitive3DReplaceChunkRefs {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                })
            }
            Opcode::Primitive3DUpsertMaterials => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let count = self.read_u32();
                let mut materials = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let material_id = self.read_u32();
                    let material = self.read_material();
                    materials.push(Primitive3DMaterialCommand {
                        material_id,
                        material,
                    });
                }
                Some(DrawCommand::Primitive3DUpsertMaterials {
                    scene_id,
                    layer_id,
                    materials,
                })
            }
            Opcode::Primitive3DUpsertShapes => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let count = self.read_u32();
                let mut shapes = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    shapes.push(Primitive3DShapeCommand {
                        shape_id: self.read_u32(),
                        model_id: self.read_u32(),
                    });
                }
                Some(DrawCommand::Primitive3DUpsertShapes {
                    scene_id,
                    layer_id,
                    shapes,
                })
            }
            Opcode::Primitive3DReplaceChunkKeys => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let chunk_id = self.read_u32();
                let count = self.read_u32();
                let mut instances = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    instances.push(self.read_primitive3d_instance_key());
                }
                Some(DrawCommand::Primitive3DReplaceChunkKeys {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                })
            }
            Opcode::Primitive3DSetChunkVisible => {
                let scene_id = self.read_u32();
                let layer_id = self.read_u32();
                let chunk_id = self.read_u32();
                let visible = self.read_u8() != 0;
                Some(DrawCommand::Primitive3DSetChunkVisible {
                    scene_id,
                    layer_id,
                    chunk_id,
                    visible,
                })
            }
            Opcode::DrawBillboard => {
                let tex_id = self.read_u32();
                let src_x = self.read_f32();
                let src_y = self.read_f32();
                let src_w = self.read_f32();
                let src_h = self.read_f32();
                let world_pos = Vec3::new(self.read_f32(), self.read_f32(), self.read_f32());
                let w = self.read_f32();
                let h = self.read_f32();
                let tint = [
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                    self.read_f32(),
                ];
                Some(DrawCommand::DrawBillboard {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    world_pos,
                    w,
                    h,
                    tint,
                })
            }
        }
    }
}

/// Iterate all commands in a stream.
#[allow(dead_code)]
pub fn decode_all(data: &[u8]) -> Vec<DrawCommand> {
    let mut reader = StreamReader::new(data);
    let mut commands = Vec::new();
    while let Some(cmd) = reader.next_command() {
        commands.push(cmd);
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::{decode_all, DrawCommand};

    fn push_default_material(data: &mut Vec<u8>) {
        data.extend_from_slice(&0u32.to_le_bytes());
        for value in [1.0f32, 1.0, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u32, 0, 0, 0, 0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 0.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.55f32, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        for value in [1.0f32, 0.0, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u32, 0, 0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn push_primitive_instance(data: &mut Vec<u8>, object_id: u32, model_id: u32, visible: bool) {
        data.extend_from_slice(&object_id.to_le_bytes());
        data.extend_from_slice(&model_id.to_le_bytes());
        for value in [0.0f32, 1.0, 2.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1.0f32, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        push_default_material(data);
        data.push(if visible { 1 } else { 0 });
        push_primitive_render_params(data);
    }

    fn push_primitive_instance_ref(
        data: &mut Vec<u8>,
        object_id: u32,
        model_id: u32,
        material_id: u32,
        visible: bool,
    ) {
        data.extend_from_slice(&object_id.to_le_bytes());
        data.extend_from_slice(&model_id.to_le_bytes());
        for value in [0.0f32, 1.0, 2.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1.0f32, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&material_id.to_le_bytes());
        data.push(if visible { 1 } else { 0 });
        push_primitive_render_params(data);
    }

    fn push_primitive_instance_key(
        data: &mut Vec<u8>,
        object_id: u32,
        shape_id: u32,
        material_id: u32,
        visible: bool,
    ) {
        data.extend_from_slice(&object_id.to_le_bytes());
        data.extend_from_slice(&shape_id.to_le_bytes());
        for value in [0.0f32, 1.0, 2.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1.0f32, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&material_id.to_le_bytes());
        for value in [0.0f32, 0.0, 0.0, 0.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.push(if visible { 1 } else { 0 });
        push_primitive_render_params(data);
    }

    fn push_primitive_render_params(data: &mut Vec<u8>) {
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&12.0f32.to_le_bytes());
        data.extend_from_slice(&96.0f32.to_le_bytes());
        data.extend_from_slice(&0.35f32.to_le_bytes());
        for value in [0.25f32, 0.5, 0.125, 0.25] {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn decodes_color_grading_command() {
        let mut data = Vec::new();
        data.push(0x26);
        data.push(2);
        data.extend_from_slice(&1.08f32.to_le_bytes());
        data.extend_from_slice(&1.04f32.to_le_bytes());
        data.extend_from_slice(&1.06f32.to_le_bytes());
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetColorGrading3D {
                tone_map,
                exposure,
                contrast,
                saturation,
            } => {
                assert_eq!(*tone_map, 2);
                assert!((*exposure - 1.08).abs() < 0.0001);
                assert!((*contrast - 1.04).abs() < 0.0001);
                assert!((*saturation - 1.06).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_hemisphere_ambient_lights_command() {
        let mut data = Vec::new();
        data.push(0x23);
        data.extend_from_slice(&0.52f32.to_le_bytes());
        data.extend_from_slice(&0.64f32.to_le_bytes());
        data.extend_from_slice(&0.70f32.to_le_bytes());
        data.extend_from_slice(&0.22f32.to_le_bytes());
        data.extend_from_slice(&0.30f32.to_le_bytes());
        data.extend_from_slice(&0.30f32.to_le_bytes());
        data.push(0);
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetLights3D {
                ambient_r,
                ambient_g,
                ambient_b,
                ambient_ground_r,
                ambient_ground_g,
                ambient_ground_b,
                lights,
            } => {
                assert!((*ambient_r - 0.52).abs() < 0.0001);
                assert!((*ambient_g - 0.64).abs() < 0.0001);
                assert!((*ambient_b - 0.70).abs() < 0.0001);
                assert!((*ambient_ground_r - 0.22).abs() < 0.0001);
                assert!((*ambient_ground_g - 0.30).abs() < 0.0001);
                assert!((*ambient_ground_b - 0.30).abs() < 0.0001);
                assert!(lights.is_empty());
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_render_debug_command() {
        let commands = decode_all(&[0x28, 3]);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetRenderDebug3D { mode } => assert_eq!(*mode, 3),
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_post_process_command() {
        let mut data = Vec::new();
        data.push(0x29);
        data.extend_from_slice(&0.72f32.to_le_bytes());
        data.extend_from_slice(&0.18f32.to_le_bytes());
        data.extend_from_slice(&0.06f32.to_le_bytes());
        data.extend_from_slice(&0.95f32.to_le_bytes());
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetPostProcess3D {
                bloom_threshold,
                bloom_strength,
                sharpen_strength,
                fxaa_strength,
            } => {
                assert!((*bloom_threshold - 0.72).abs() < 0.0001);
                assert!((*bloom_strength - 0.18).abs() < 0.0001);
                assert!((*sharpen_strength - 0.06).abs() < 0.0001);
                assert!((*fxaa_strength - 0.95).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_contact_ao_command() {
        let mut data = Vec::new();
        data.push(0x2A);
        data.extend_from_slice(&0.38f32.to_le_bytes());
        data.extend_from_slice(&2.75f32.to_le_bytes());
        data.extend_from_slice(&84.0f32.to_le_bytes());
        data.extend_from_slice(&0.22f32.to_le_bytes());
        data.extend_from_slice(&1.15f32.to_le_bytes());
        data.extend_from_slice(&0.018f32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetContactAO3D {
                strength,
                radius,
                depth_scale,
                detail_strength,
                detail_radius,
                normal_bias,
                quality,
            } => {
                assert!((*strength - 0.38).abs() < 0.0001);
                assert!((*radius - 2.75).abs() < 0.0001);
                assert!((*depth_scale - 84.0).abs() < 0.0001);
                assert!((*detail_strength - 0.22).abs() < 0.0001);
                assert!((*detail_radius - 1.15).abs() < 0.0001);
                assert!((*normal_bias - 0.018).abs() < 0.0001);
                assert_eq!(*quality, 3);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_projected_decal_command() {
        let mut data = Vec::new();
        data.push(0x22);
        for value in [1.0f32, 2.0, 3.0, 0.75, 4.0, 5.0, 1.5, 0.2, 0.3, 0.4, 0.65] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::DrawProjectedDecal3D {
                position,
                yaw,
                width,
                length,
                depth,
                color,
            } => {
                assert!((position.x - 1.0).abs() < 0.0001);
                assert!((position.y - 2.0).abs() < 0.0001);
                assert!((position.z - 3.0).abs() < 0.0001);
                assert!((*yaw - 0.75).abs() < 0.0001);
                assert!((*width - 4.0).abs() < 0.0001);
                assert!((*length - 5.0).abs() < 0.0001);
                assert!((*depth - 1.5).abs() < 0.0001);
                assert!((color[3] - 0.65).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_projected_decal_atlas_commands() {
        let mut data = Vec::new();
        data.push(0x2B);
        data.extend_from_slice(&77u32.to_le_bytes());
        data.push(0x40);
        data.extend_from_slice(&88u32.to_le_bytes());
        data.push(0x41);
        data.extend_from_slice(&99u32.to_le_bytes());
        data.push(0x42);
        data.extend_from_slice(&100u32.to_le_bytes());
        data.push(0x2D);
        data.extend_from_slice(&18.0f32.to_le_bytes());
        data.extend_from_slice(&34.0f32.to_le_bytes());
        data.push(0x43);
        data.extend_from_slice(&0.35f32.to_le_bytes());
        data.extend_from_slice(&0.7f32.to_le_bytes());
        data.push(0x2E);
        data.extend_from_slice(&1u32.to_le_bytes());
        data.push(0x2F);
        data.extend_from_slice(&0.8f32.to_le_bytes());
        data.extend_from_slice(&0.38f32.to_le_bytes());
        data.extend_from_slice(&0.7f32.to_le_bytes());
        data.push(0x2C);
        for value in [
            1.0f32, 2.0, 3.0, 0.75, 4.0, 5.0, 1.5, 0.2, 0.3, 0.4, 0.65, 0.1, 0.2, 0.25, 0.5,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 9);
        match &commands[0] {
            DrawCommand::SetProjectedDecalAtlas3D { atlas_id } => assert_eq!(*atlas_id, 77),
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[1] {
            DrawCommand::SetProjectedDecalNormalAtlas3D { atlas_id } => {
                assert_eq!(*atlas_id, 88)
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[2] {
            DrawCommand::SetProjectedDecalRoughnessAtlas3D { atlas_id } => {
                assert_eq!(*atlas_id, 99)
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[3] {
            DrawCommand::SetProjectedDecalMaskAtlas3D { atlas_id } => {
                assert_eq!(*atlas_id, 100)
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[4] {
            DrawCommand::SetProjectedDecalDistanceFade3D { start, end } => {
                assert!((*start - 18.0).abs() < 0.0001);
                assert!((*end - 34.0).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[5] {
            DrawCommand::SetProjectedDecalAngleFade3D { start, end } => {
                assert!((*start - 0.35).abs() < 0.0001);
                assert!((*end - 0.7).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[6] {
            DrawCommand::SetProjectedDecalReceiverMask3D { mask } => assert_eq!(*mask, 1),
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[7] {
            DrawCommand::SetProjectedDecalSurfaceResponse3D {
                normal_strength,
                roughness,
                roughness_strength,
            } => {
                assert!((*normal_strength - 0.8).abs() < 0.0001);
                assert!((*roughness - 0.38).abs() < 0.0001);
                assert!((*roughness_strength - 0.7).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[8] {
            DrawCommand::DrawProjectedDecal3DUV {
                position,
                uv_rect,
                color,
                ..
            } => {
                assert!((position.x - 1.0).abs() < 0.0001);
                assert!((color[3] - 0.65).abs() < 0.0001);
                assert!((uv_rect[0] - 0.1).abs() < 0.0001);
                assert!((uv_rect[3] - 0.5).abs() < 0.0001);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_shadow_strength_command() {
        let mut data = Vec::new();
        data.push(0x27);
        data.push(1);
        data.extend_from_slice(&2048u32.to_le_bytes());
        data.extend_from_slice(&0.58f32.to_le_bytes());
        data.extend_from_slice(&1.75f32.to_le_bytes());
        data.extend_from_slice(&260.0f32.to_le_bytes());
        data.extend_from_slice(&70.0f32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::SetShadow3D {
                enabled,
                resolution,
                strength,
                softness,
                distance,
                fade,
                quality,
            } => {
                assert!(*enabled);
                assert_eq!(*resolution, 2048);
                assert!((*strength - 0.58).abs() < 0.0001);
                assert!((*softness - 1.75).abs() < 0.0001);
                assert!((*distance - 260.0).abs() < 0.0001);
                assert!((*fade - 70.0).abs() < 0.0001);
                assert_eq!(*quality, 3);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_primitive_chunk_replace_command() {
        let mut data = Vec::new();
        data.push(0x38);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        push_primitive_instance(&mut data, 11, 21, true);
        push_primitive_instance(&mut data, 12, 22, false);
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::Primitive3DReplaceChunk {
                scene_id,
                layer_id,
                chunk_id,
                instances,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(*chunk_id, 9);
                assert_eq!(instances.len(), 2);
                assert_eq!(instances[0].object_id, 11);
                assert_eq!(instances[0].model_id, 21);
                assert!(instances[0].visible);
                assert_eq!(instances[0].flags, 2);
                assert!((instances[0].lod_near - 12.0).abs() < 0.0001);
                assert!((instances[0].lod_far - 96.0).abs() < 0.0001);
                assert!((instances[0].wind_strength - 0.35).abs() < 0.0001);
                assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
                assert!(!instances[1].visible);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_primitive_material_table_and_chunk_refs() {
        let mut data = Vec::new();
        data.push(0x3A);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&33u32.to_le_bytes());
        push_default_material(&mut data);
        data.push(0x39);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        push_primitive_instance_ref(&mut data, 11, 21, 33, true);
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 2);
        match &commands[0] {
            DrawCommand::Primitive3DUpsertMaterials {
                scene_id,
                layer_id,
                materials,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(materials.len(), 1);
                assert_eq!(materials[0].material_id, 33);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[1] {
            DrawCommand::Primitive3DReplaceChunkRefs {
                scene_id,
                layer_id,
                chunk_id,
                instances,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(*chunk_id, 9);
                assert_eq!(instances.len(), 1);
                assert_eq!(instances[0].material_id, 33);
                assert!(instances[0].visible);
                assert_eq!(instances[0].flags, 2);
                assert!((instances[0].wind_strength - 0.35).abs() < 0.0001);
                assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_primitive_shape_material_tables_and_chunk_keys() {
        let mut data = Vec::new();
        data.push(0x3B);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&44u32.to_le_bytes());
        data.extend_from_slice(&21u32.to_le_bytes());
        data.push(0x3A);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&33u32.to_le_bytes());
        push_default_material(&mut data);
        data.push(0x3C);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        push_primitive_instance_key(&mut data, 11, 44, 33, true);
        data.push(0x3D);
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.push(0);
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 4);
        match &commands[0] {
            DrawCommand::Primitive3DUpsertShapes {
                scene_id,
                layer_id,
                shapes,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(shapes.len(), 1);
                assert_eq!(shapes[0].shape_id, 44);
                assert_eq!(shapes[0].model_id, 21);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[2] {
            DrawCommand::Primitive3DReplaceChunkKeys {
                scene_id,
                layer_id,
                chunk_id,
                instances,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(*chunk_id, 9);
                assert_eq!(instances.len(), 1);
                assert_eq!(instances[0].shape_id, 44);
                assert_eq!(instances[0].material_id, 33);
                assert_eq!(instances[0].tint, [0.0, 0.0, 0.0, 0.0]);
                assert!(instances[0].visible);
                assert_eq!(instances[0].flags, 2);
                assert!((instances[0].lod_far - 96.0).abs() < 0.0001);
                assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
            }
            other => panic!("unexpected command: {:?}", other),
        }
        match &commands[3] {
            DrawCommand::Primitive3DSetChunkVisible {
                scene_id,
                layer_id,
                chunk_id,
                visible,
            } => {
                assert_eq!(*scene_id, 7);
                assert_eq!(*layer_id, 8);
                assert_eq!(*chunk_id, 9);
                assert!(!*visible);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn decodes_extended_material_command() {
        let mut data = Vec::new();
        data.push(0x20);
        data.extend_from_slice(&7u32.to_le_bytes());
        for value in [1.0f32, 2.0, 3.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1.0f32, 1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&9u32.to_le_bytes());
        for value in [0.2f32, 0.3, 0.4, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&11u32.to_le_bytes());
        data.extend_from_slice(&12u32.to_le_bytes());
        data.extend_from_slice(&13u32.to_le_bytes());
        data.extend_from_slice(&14u32.to_le_bytes());
        data.extend_from_slice(&16u32.to_le_bytes());
        for value in [0.1f32, 0.2, 0.3, 1.1] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&0.72f32.to_le_bytes());
        data.extend_from_slice(&0.28f32.to_le_bytes());
        data.extend_from_slice(&0.66f32.to_le_bytes());
        data.extend_from_slice(&3.5f32.to_le_bytes());
        data.extend_from_slice(&15u32.to_le_bytes());
        data.extend_from_slice(&1.25f32.to_le_bytes());
        data.extend_from_slice(&0.45f32.to_le_bytes());
        data.extend_from_slice(&0.8f32.to_le_bytes());
        data.extend_from_slice(&0.6f32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&21u32.to_le_bytes());
        data.extend_from_slice(&22u32.to_le_bytes());
        let commands = decode_all(&data);
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            DrawCommand::DrawModel {
                model_id,
                material,
                animation_world_id,
                animation_target_id,
                ..
            } => {
                assert_eq!(*model_id, 7);
                assert_eq!(material.id, 9);
                assert_eq!(material.albedo_texture_id, 11);
                assert_eq!(material.normal_texture_id, 12);
                assert_eq!(material.metallic_roughness_texture_id, 13);
                assert_eq!(material.emissive_texture_id, 14);
                assert_eq!(material.mask_texture_id, 16);
                assert!((material.roughness - 0.72).abs() < 0.0001);
                assert!((material.metallic - 0.28).abs() < 0.0001);
                assert!((material.normal_scale - 0.66).abs() < 0.0001);
                assert!((material.uv_scale - 3.5).abs() < 0.0001);
                assert_eq!(material.toon_ramp_texture_id, 15);
                assert!((material.detail_strength - 1.25).abs() < 0.0001);
                assert!((material.macro_blend - 0.45).abs() < 0.0001);
                assert!((material.roughness_response - 0.8).abs() < 0.0001);
                assert!((material.toon_ramp_response - 0.6).abs() < 0.0001);
                assert_eq!(material.shading_mode, 1);
                assert_eq!(material.wrap_mode, 2);
                assert_eq!(material.filter_mode, 1);
                assert_eq!(*animation_world_id, 21);
                assert_eq!(*animation_target_id, 22);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }
}
