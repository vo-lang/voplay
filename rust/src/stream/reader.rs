use super::*;

pub struct StreamReader<'a> {
    data: &'a [u8],
    pos: usize,
    base_offset: usize,
    error: Option<DrawStreamError>,
}

impl<'a> StreamReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, DrawStreamError> {
        if data.len() < DRAW_STREAM_HEADER_SIZE {
            return Err(DrawStreamError::HeaderTooShort { actual: data.len() });
        }
        let actual_magic = [data[0], data[1], data[2], data[3]];
        if actual_magic != DRAW_STREAM_MAGIC {
            return Err(DrawStreamError::InvalidMagic {
                actual: actual_magic,
            });
        }
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != DRAW_STREAM_VERSION {
            return Err(DrawStreamError::UnsupportedVersion { actual: version });
        }
        let flags = u16::from_le_bytes([data[6], data[7]]);
        if flags != DRAW_STREAM_FLAGS {
            return Err(DrawStreamError::UnsupportedFlags { actual: flags });
        }
        let declared = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let payload = &data[DRAW_STREAM_HEADER_SIZE..];
        if declared != payload.len() {
            return Err(DrawStreamError::PayloadLengthMismatch {
                declared,
                actual: payload.len(),
            });
        }
        Ok(Self {
            data: payload,
            pos: 0,
            base_offset: DRAW_STREAM_HEADER_SIZE,
            error: None,
        })
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
        self.data.len().saturating_sub(self.pos)
    }

    fn stream_offset(&self) -> usize {
        self.base_offset.saturating_add(self.pos)
    }

    fn check_remaining(&mut self, needed: usize) -> bool {
        let remaining = self.remaining();
        if remaining >= needed {
            return true;
        }
        if self.error.is_none() {
            self.error = Some(DrawStreamError::Truncated {
                offset: self.stream_offset(),
                needed,
                remaining,
            });
        }
        false
    }

    fn validate_count(&mut self, count: u32, item_size: usize) -> usize {
        let count = count as usize;
        let remaining = self.remaining();
        let fits = count
            .checked_mul(item_size)
            .is_some_and(|needed| needed <= remaining);
        if fits {
            return count;
        }
        if self.error.is_none() {
            self.error = Some(DrawStreamError::InvalidCount {
                offset: self.stream_offset().saturating_sub(4),
                count,
                item_size,
                remaining,
            });
        }
        0
    }

    fn read_u8(&mut self) -> u8 {
        if !self.check_remaining(1) {
            return 0;
        }
        let v = self.data[self.pos];
        self.pos += 1;
        v
    }

    fn read_u16(&mut self) -> u16 {
        if !self.check_remaining(2) {
            return 0;
        }
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        v
    }

    fn read_u32(&mut self) -> u32 {
        if !self.check_remaining(4) {
            return 0;
        }
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
        if !self.check_remaining(4) {
            return 0.0;
        }
        let bytes = [
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ];
        self.pos += 4;
        f32::from_le_bytes(bytes)
    }

    /// Decode the next command from the stream.
    pub fn next_command(&mut self) -> Result<Option<DrawCommand>, DrawStreamError> {
        if self.remaining() == 0 {
            return Ok(None);
        }

        let opcode_offset = self.stream_offset();
        let op_byte = self.read_u8();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let op = Opcode::from_u8(op_byte).ok_or(DrawStreamError::UnknownOpcode {
            offset: opcode_offset,
            opcode: op_byte,
        })?;

        let command = match op {
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
                let text = if self.check_remaining(len) {
                    let text =
                        String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
                    self.pos += len;
                    text
                } else {
                    String::new()
                };
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
                let raw_count = self.read_u32();
                let count = self.validate_count(raw_count, 185);
                let mut instances = Vec::with_capacity(count);
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
                let raw_count = self.read_u32();
                let count = self.validate_count(raw_count, 85);
                let mut instances = Vec::with_capacity(count);
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
                let raw_count = self.read_u32();
                let count = self.validate_count(raw_count, 108);
                let mut materials = Vec::with_capacity(count);
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
                let raw_count = self.read_u32();
                let count = self.validate_count(raw_count, 8);
                let mut shapes = Vec::with_capacity(count);
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
                let raw_count = self.read_u32();
                let count = self.validate_count(raw_count, 101);
                let mut instances = Vec::with_capacity(count);
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
        };
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        Ok(command)
    }
}
