//! Binary draw command stream decoder.
//! Reads opcodes + args from the []byte buffer produced by DrawCtx on the Vo side.

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
    SetLights3D = 0x23,
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
            0x23 => Some(Opcode::SetLights3D),
            _ => None,
        }
    }
}

/// Decoded light from SetLights3D command.
#[derive(Debug, Clone)]
pub struct DecodedLight {
    pub light_type: u8, // 0 = directional, 1 = point
    pub px: f32, pub py: f32, pub pz: f32, // position (point) or unused (dir)
    pub dx: f32, pub dy: f32, pub dz: f32, // direction (dir) or unused (point)
    pub cr: f32, pub cg: f32, pub cb: f32, // color
    pub intensity: f32,
}

/// Decoded draw command.
#[derive(Debug)]
#[allow(dead_code)]
pub enum DrawCommand {
    Clear { r: f32, g: f32, b: f32, a: f32 },
    SetCamera2D { x: f32, y: f32, zoom: f32, rotation: f32 },
    ResetCamera,
    SetLayer { z: u16 },
    DrawRect { x: f32, y: f32, w: f32, h: f32, r: f32, g: f32, b: f32, a: f32 },
    DrawCircle { cx: f32, cy: f32, radius: f32, r: f32, g: f32, b: f32, a: f32 },
    DrawLine { x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, r: f32, g: f32, b: f32, a: f32 },
    DrawText { x: f32, y: f32, size: f32, r: f32, g: f32, b: f32, a: f32, text: String },
    SetFont { font_id: u32 },
    SetCamera3D {
        eye_x: f32, eye_y: f32, eye_z: f32,
        target_x: f32, target_y: f32, target_z: f32,
        up_x: f32, up_y: f32, up_z: f32,
        fov: f32, near: f32, far: f32,
    },
    SetLights3D {
        ambient_r: f32, ambient_g: f32, ambient_b: f32,
        lights: Vec<DecodedLight>,
    },
    DrawModel {
        model_id: u32,
        px: f32, py: f32, pz: f32,
        qx: f32, qy: f32, qz: f32, qw: f32,
        sx: f32, sy: f32, sz: f32,
    },
    DrawSprite {
        tex_id: u32,
        src_x: f32, src_y: f32, src_w: f32, src_h: f32,
        dst_x: f32, dst_y: f32, dst_w: f32, dst_h: f32,
        flip_x: bool, flip_y: bool,
        rotation: f32,
        r: f32, g: f32, b: f32, a: f32,
    },
    DrawBillboard {
        tex_id: u32,
        src_x: f32, src_y: f32, src_w: f32, src_h: f32,
        world_x: f32, world_y: f32, world_z: f32,
        w: f32, h: f32,
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

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn check_remaining(&self, n: usize) {
        assert!(
            self.remaining() >= n,
            "voplay: draw stream truncated at pos {} (need {} bytes, have {})",
            self.pos, n, self.remaining()
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
                Some(DrawCommand::SetCamera2D { x, y, zoom, rotation })
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
                Some(DrawCommand::DrawRect { x, y, w, h, r, g, b, a })
            }
            Opcode::DrawCircle => {
                let cx = self.read_f32();
                let cy = self.read_f32();
                let radius = self.read_f32();
                let r = self.read_f32();
                let g = self.read_f32();
                let b = self.read_f32();
                let a = self.read_f32();
                Some(DrawCommand::DrawCircle { cx, cy, radius, r, g, b, a })
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
                Some(DrawCommand::DrawLine { x1, y1, x2, y2, thickness, r, g, b, a })
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
                let text = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
                self.pos += len;
                Some(DrawCommand::DrawText { x, y, size, r, g, b, a, text })
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
                    tex_id, src_x, src_y, src_w, src_h,
                    dst_x, dst_y, dst_w, dst_h,
                    flip_x, flip_y, rotation,
                    r, g, b, a,
                })
            }
            Opcode::SetCamera3D => {
                let eye_x = self.read_f32(); let eye_y = self.read_f32(); let eye_z = self.read_f32();
                let target_x = self.read_f32(); let target_y = self.read_f32(); let target_z = self.read_f32();
                let up_x = self.read_f32(); let up_y = self.read_f32(); let up_z = self.read_f32();
                let fov = self.read_f32(); let near = self.read_f32(); let far = self.read_f32();
                Some(DrawCommand::SetCamera3D {
                    eye_x, eye_y, eye_z,
                    target_x, target_y, target_z,
                    up_x, up_y, up_z,
                    fov, near, far,
                })
            }
            Opcode::SetLights3D => {
                let ambient_r = self.read_f32();
                let ambient_g = self.read_f32();
                let ambient_b = self.read_f32();
                let count = self.read_u8() as usize;
                let mut lights = Vec::with_capacity(count);
                for _ in 0..count {
                    let light_type = self.read_u8();
                    let px = self.read_f32(); let py = self.read_f32(); let pz = self.read_f32();
                    let dx = self.read_f32(); let dy = self.read_f32(); let dz = self.read_f32();
                    let cr = self.read_f32(); let cg = self.read_f32(); let cb = self.read_f32();
                    let intensity = self.read_f32();
                    lights.push(DecodedLight {
                        light_type, px, py, pz, dx, dy, dz, cr, cg, cb, intensity,
                    });
                }
                Some(DrawCommand::SetLights3D { ambient_r, ambient_g, ambient_b, lights })
            }
            Opcode::DrawModel => {
                let model_id = self.read_u32();
                let px = self.read_f32(); let py = self.read_f32(); let pz = self.read_f32();
                let qx = self.read_f32(); let qy = self.read_f32(); let qz = self.read_f32(); let qw = self.read_f32();
                let sx = self.read_f32(); let sy = self.read_f32(); let sz = self.read_f32();
                Some(DrawCommand::DrawModel {
                    model_id, px, py, pz, qx, qy, qz, qw, sx, sy, sz,
                })
            }
            Opcode::DrawBillboard => {
                let tex_id = self.read_u32();
                let src_x = self.read_f32();
                let src_y = self.read_f32();
                let src_w = self.read_f32();
                let src_h = self.read_f32();
                let world_x = self.read_f32();
                let world_y = self.read_f32();
                let world_z = self.read_f32();
                let w = self.read_f32();
                let h = self.read_f32();
                Some(DrawCommand::DrawBillboard {
                    tex_id, src_x, src_y, src_w, src_h,
                    world_x, world_y, world_z, w, h,
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
