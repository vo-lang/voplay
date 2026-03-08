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
    DrawSprite = 0x10,
    DrawRect = 0x11,
    DrawCircle = 0x12,
    DrawLine = 0x13,
    DrawText = 0x14,
    DrawTilemap = 0x15,
    DrawScene2D = 0x16,
    DrawModel = 0x20,
    DrawBillboard = 0x21,
    DrawScene3D = 0x22,
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
            0x10 => Some(Opcode::DrawSprite),
            0x11 => Some(Opcode::DrawRect),
            0x12 => Some(Opcode::DrawCircle),
            0x13 => Some(Opcode::DrawLine),
            0x14 => Some(Opcode::DrawText),
            0x15 => Some(Opcode::DrawTilemap),
            0x16 => Some(Opcode::DrawScene2D),
            0x20 => Some(Opcode::DrawModel),
            0x21 => Some(Opcode::DrawBillboard),
            0x22 => Some(Opcode::DrawScene3D),
            0x23 => Some(Opcode::SetLights3D),
            _ => None,
        }
    }
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
    // TODO: Phase 2+ — DrawSprite, DrawTilemap, DrawModel, etc.
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

    fn read_u8(&mut self) -> u8 {
        let v = self.data[self.pos];
        self.pos += 1;
        v
    }

    fn read_u16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        v
    }

    fn read_f32(&mut self) -> f32 {
        // Wire format is f64 (Vo has no Float32bits). Read f64, truncate to f32.
        let bytes = [
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ];
        self.pos += 8;
        f64::from_le_bytes(bytes) as f32
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
                let text = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
                self.pos += len;
                Some(DrawCommand::DrawText { x, y, size, r, g, b, a, text })
            }
            // Unimplemented opcodes — skip for now
            _ => {
                log::warn!("voplay: unhandled opcode {:?}", op);
                None
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
