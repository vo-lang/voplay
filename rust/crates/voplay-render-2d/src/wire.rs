#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireAtlasRegion2d {
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireSprite2d {
    pub id: u64,
    pub image: u64,
    pub atlas: WireAtlasRegion2d,
    pub position_milli: [i64; 2],
    pub size_milli: [u32; 2],
    pub anchor_milli: [i32; 2],
    pub color: [u8; 4],
    pub z: i64,
    pub layers: u64,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireShapeKind2d {
    Rect,
    RoundedRect,
    Circle,
    Line,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireShape2d {
    pub id: u64,
    pub kind: WireShapeKind2d,
    pub bounds_milli: [i64; 4],
    pub radius_milli: u32,
    pub stroke_milli: u32,
    pub fill: [u8; 4],
    pub stroke: [u8; 4],
    pub path_milli: Vec<i64>,
    pub z: i64,
    pub layers: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireText2d {
    pub id: u64,
    pub font: u64,
    pub text: Vec<u8>,
    pub position_milli: [i64; 2],
    pub size_milli: u32,
    pub color: [u8; 4],
    pub max_width_milli: u32,
    pub alignment: u32,
    pub z: i64,
    pub layers: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireTileCell2d {
    pub x: u32,
    pub y: u32,
    pub tile: u32,
    pub flags: u16,
    pub tint: [u8; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireTileChunk2d {
    pub chunk: u64,
    pub revision: u64,
    pub origin_milli: [i64; 2],
    pub cell_size_milli: [u32; 2],
    pub width: u32,
    pub height: u32,
    pub cells: Vec<WireTileCell2d>,
    pub removed: Vec<u64>,
    pub layers: u64,
    pub z: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireParticleEmitter2d {
    pub id: u64,
    pub seed: u64,
    pub image: u64,
    pub max_particles: u32,
    pub spawn_per_tick: u32,
    pub lifetime_ticks: u32,
    pub position_milli: [i64; 2],
    pub velocity_min_milli: [i64; 2],
    pub velocity_max_milli: [i64; 2],
    pub start_color: [u8; 4],
    pub end_color: [u8; 4],
    pub start_size_milli: u32,
    pub end_size_milli: u32,
    pub layers: u64,
    pub z: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireView2d {
    pub id: u64,
    pub target: u64,
    pub camera_milli: [i64; 2],
    pub zoom_milli: i32,
    pub viewport: [u32; 4],
    pub layers: u64,
    pub clear_color: [u8; 4],
    pub post_effects: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wire2dError {
    Truncated,
    TrailingBytes,
    InvalidMagic,
    InvalidDescriptor,
    Capacity,
    InvalidUtf8,
}

pub fn decode_view_2d(bytes: &[u8], max_post_effects: usize) -> Result<WireView2d, Wire2dError> {
    if bytes.len() < 4 {
        return Err(Wire2dError::Truncated);
    }
    if &bytes[..4] != b"VV21" {
        return Err(Wire2dError::InvalidMagic);
    }
    if bytes.len() < 72 {
        return Err(Wire2dError::Truncated);
    }
    let mut reader = Reader::new(&bytes[4..]);
    let view = WireView2d {
        id: reader.u64()?,
        target: reader.u64()?,
        camera_milli: [reader.i64()?, reader.i64()?],
        zoom_milli: reader.i32()?,
        viewport: [reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?],
        layers: reader.u64()?,
        clear_color: reader.array()?,
        post_effects: {
            let count = reader.count(max_post_effects)?;
            let mut effects = Vec::with_capacity(count);
            for _ in 0..count {
                effects.push(reader.u64()?);
            }
            effects
        },
    };
    reader.finish()?;
    if view.id == 0
        || view.target == 0
        || view.zoom_milli <= 0
        || view.viewport[2] == 0
        || view.viewport[3] == 0
        || view.layers == 0
        || view.post_effects.contains(&0)
        || has_duplicates(&view.post_effects)
    {
        return Err(Wire2dError::InvalidDescriptor);
    }
    Ok(view)
}

pub fn decode_sprite_2d(bytes: &[u8]) -> Result<WireSprite2d, Wire2dError> {
    if bytes.len() != 101 || &bytes[..4] != b"VS21" {
        return Err(if bytes.len() < 4 {
            Wire2dError::Truncated
        } else {
            Wire2dError::InvalidMagic
        });
    }
    let mut reader = Reader::new(&bytes[4..]);
    let sprite = WireSprite2d {
        id: reader.u64()?,
        image: reader.u64()?,
        atlas: WireAtlasRegion2d {
            page: reader.u32()?,
            x: reader.u32()?,
            y: reader.u32()?,
            width: reader.u32()?,
            height: reader.u32()?,
            revision: reader.u64()?,
        },
        position_milli: [reader.i64()?, reader.i64()?],
        size_milli: [reader.u32()?, reader.u32()?],
        anchor_milli: [reader.i32()?, reader.i32()?],
        color: reader.array()?,
        z: reader.i64()?,
        layers: reader.u64()?,
        visible: reader.boolean()?,
    };
    reader.finish()?;
    if sprite.id == 0
        || sprite.image == 0
        || sprite.size_milli.contains(&0)
        || sprite.layers == 0
        || sprite.atlas.revision == 0
        || (sprite.atlas.page != 0 && (sprite.atlas.width == 0 || sprite.atlas.height == 0))
    {
        return Err(Wire2dError::InvalidDescriptor);
    }
    Ok(sprite)
}

pub fn decode_shape_2d(bytes: &[u8], max_path_values: usize) -> Result<WireShape2d, Wire2dError> {
    if bytes.len() < 81 || &bytes[..4] != b"VH21" {
        return Err(Wire2dError::InvalidMagic);
    }
    let mut reader = Reader::new(&bytes[4..]);
    let kind = match reader.u8()? {
        1 => WireShapeKind2d::Rect,
        2 => WireShapeKind2d::RoundedRect,
        3 => WireShapeKind2d::Circle,
        4 => WireShapeKind2d::Line,
        5 => WireShapeKind2d::Path,
        _ => return Err(Wire2dError::InvalidDescriptor),
    };
    let id = reader.u64()?;
    let bounds_milli = [reader.i64()?, reader.i64()?, reader.i64()?, reader.i64()?];
    let radius_milli = reader.u32()?;
    let stroke_milli = reader.u32()?;
    let fill = reader.array()?;
    let stroke = reader.array()?;
    let z = reader.i64()?;
    let layers = reader.u64()?;
    let count = reader.count(max_path_values)?;
    if count % 2 != 0 {
        return Err(Wire2dError::InvalidDescriptor);
    }
    let mut path_milli = Vec::with_capacity(count);
    for _ in 0..count {
        path_milli.push(reader.i64()?);
    }
    reader.finish()?;
    if id == 0
        || bounds_milli[2] <= 0
        || bounds_milli[3] <= 0
        || layers == 0
        || (kind == WireShapeKind2d::Path && path_milli.len() < 4)
    {
        return Err(Wire2dError::InvalidDescriptor);
    }
    Ok(WireShape2d {
        id,
        kind,
        bounds_milli,
        radius_milli,
        stroke_milli,
        fill,
        stroke,
        path_milli,
        z,
        layers,
    })
}

pub fn decode_text_2d(bytes: &[u8], max_text_bytes: usize) -> Result<WireText2d, Wire2dError> {
    if bytes.len() < 72 || &bytes[..4] != b"VT21" {
        return Err(Wire2dError::InvalidMagic);
    }
    let mut reader = Reader::new(&bytes[4..]);
    let id = reader.u64()?;
    let font = reader.u64()?;
    let position_milli = [reader.i64()?, reader.i64()?];
    let size_milli = reader.u32()?;
    let max_width_milli = reader.u32()?;
    let alignment = reader.u32()?;
    let z = reader.i64()?;
    let layers = reader.u64()?;
    let color = reader.array()?;
    let length = reader.count(max_text_bytes)?;
    let text = reader.bytes(length)?.to_vec();
    reader.finish()?;
    if id == 0 || font == 0 || text.is_empty() || size_milli == 0 || alignment > 3 || layers == 0 {
        return Err(Wire2dError::InvalidDescriptor);
    }
    std::str::from_utf8(&text).map_err(|_| Wire2dError::InvalidUtf8)?;
    Ok(WireText2d {
        id,
        font,
        text,
        position_milli,
        size_milli,
        color,
        max_width_milli,
        alignment,
        z,
        layers,
    })
}

pub fn decode_tile_chunk_2d(
    bytes: &[u8],
    max_cells: usize,
    max_removed: usize,
) -> Result<WireTileChunk2d, Wire2dError> {
    if bytes.len() < 76 || &bytes[..4] != b"VC21" {
        return Err(Wire2dError::InvalidMagic);
    }
    let mut reader = Reader::new(&bytes[4..]);
    let chunk = reader.u64()?;
    let revision = reader.u64()?;
    let origin_milli = [reader.i64()?, reader.i64()?];
    let cell_size_milli = [reader.u32()?, reader.u32()?];
    let width = reader.u32()?;
    let height = reader.u32()?;
    let layers = reader.u64()?;
    let z = reader.i64()?;
    let count = reader.count(max_cells)?;
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count {
        let cell = WireTileCell2d {
            x: reader.u32()?,
            y: reader.u32()?,
            tile: reader.u32()?,
            flags: reader.u16()?,
            tint: reader.array()?,
        };
        if cell.x >= width || cell.y >= height || cell.tile == 0 {
            return Err(Wire2dError::InvalidDescriptor);
        }
        cells.push(cell);
    }
    let removed_count = reader.count(max_removed)?;
    let mut removed = Vec::with_capacity(removed_count);
    for _ in 0..removed_count {
        removed.push(reader.u64()?);
    }
    reader.finish()?;
    if chunk == 0
        || revision == 0
        || cell_size_milli.contains(&0)
        || width == 0
        || height == 0
        || layers == 0
    {
        return Err(Wire2dError::InvalidDescriptor);
    }
    Ok(WireTileChunk2d {
        chunk,
        revision,
        origin_milli,
        cell_size_milli,
        width,
        height,
        cells,
        removed,
        layers,
        z,
    })
}

pub fn decode_particle_emitter_2d(bytes: &[u8]) -> Result<WireParticleEmitter2d, Wire2dError> {
    if bytes.len() != 120 || &bytes[..4] != b"VP21" {
        return Err(Wire2dError::InvalidMagic);
    }
    let mut reader = Reader::new(&bytes[4..]);
    let emitter = WireParticleEmitter2d {
        id: reader.u64()?,
        seed: reader.u64()?,
        image: reader.u64()?,
        max_particles: reader.u32()?,
        spawn_per_tick: reader.u32()?,
        lifetime_ticks: reader.u32()?,
        position_milli: [reader.i64()?, reader.i64()?],
        velocity_min_milli: [reader.i64()?, reader.i64()?],
        velocity_max_milli: [reader.i64()?, reader.i64()?],
        start_color: reader.array()?,
        end_color: reader.array()?,
        start_size_milli: reader.u32()?,
        end_size_milli: reader.u32()?,
        layers: reader.u64()?,
        z: reader.i64()?,
    };
    reader.finish()?;
    if emitter.id == 0
        || emitter.seed == 0
        || emitter.image == 0
        || emitter.max_particles == 0
        || emitter.spawn_per_tick == 0
        || emitter.spawn_per_tick > emitter.max_particles
        || emitter.lifetime_ticks == 0
        || emitter.layers == 0
        || (0..2).any(|axis| emitter.velocity_min_milli[axis] > emitter.velocity_max_milli[axis])
    {
        return Err(Wire2dError::InvalidDescriptor);
    }
    Ok(emitter)
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], Wire2dError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(Wire2dError::Capacity)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(Wire2dError::Truncated)?;
        self.cursor = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Wire2dError> {
        self.bytes(N)?
            .try_into()
            .map_err(|_| Wire2dError::Truncated)
    }

    fn u8(&mut self) -> Result<u8, Wire2dError> {
        Ok(self.bytes(1)?[0])
    }

    fn boolean(&mut self) -> Result<bool, Wire2dError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Wire2dError::InvalidDescriptor),
        }
    }

    fn u16(&mut self) -> Result<u16, Wire2dError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, Wire2dError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, Wire2dError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Wire2dError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, Wire2dError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    fn count(&mut self, maximum: usize) -> Result<usize, Wire2dError> {
        usize::try_from(self.u32()?)
            .ok()
            .filter(|count| *count <= maximum)
            .ok_or(Wire2dError::Capacity)
    }

    fn finish(self) -> Result<(), Wire2dError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Wire2dError::TrailingBytes)
        }
    }
}

fn has_duplicates(values: &[u64]) -> bool {
    values
        .iter()
        .enumerate()
        .any(|(index, value)| values[..index].contains(value))
}
