use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AtlasPageId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AtlasEntryId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtlasEntry {
    pub id: AtlasEntryId,
    pub page: AtlasPageId,
    pub rect: AtlasRect,
    pub source_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtlasConfig {
    pub page_width: u32,
    pub page_height: u32,
    pub max_pages: u32,
    pub max_entries: usize,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Feature2dError {
    Closed,
    InvalidConfig,
    Capacity,
    Duplicate,
    Unknown,
    InvalidDescriptor,
    StaleRevision,
    ArithmeticOverflow,
    TickSequence,
}

#[derive(Clone, Debug)]
struct AtlasPage {
    id: AtlasPageId,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

pub struct AtlasAllocator {
    config: AtlasConfig,
    pages: Vec<AtlasPage>,
    entries: BTreeMap<AtlasEntryId, AtlasEntry>,
    closed: bool,
}

impl AtlasAllocator {
    pub fn new(config: AtlasConfig) -> Result<Self, Feature2dError> {
        if config.page_width == 0
            || config.page_height == 0
            || config.max_pages == 0
            || config.max_entries == 0
            || config.padding.saturating_mul(2) >= config.page_width
            || config.padding.saturating_mul(2) >= config.page_height
        {
            return Err(Feature2dError::InvalidConfig);
        }
        Ok(Self {
            config,
            pages: Vec::new(),
            entries: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> (usize, usize) {
        let released = (self.pages.len(), self.entries.len());
        self.closed = true;
        self.pages.clear();
        self.entries.clear();
        released
    }

    pub fn allocate(
        &mut self,
        id: AtlasEntryId,
        width: u32,
        height: u32,
        source_revision: u64,
    ) -> Result<AtlasEntry, Feature2dError> {
        self.ensure_open()?;
        if id.0 == 0
            || width == 0
            || height == 0
            || source_revision == 0
            || self.entries.contains_key(&id)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        if self.entries.len() >= self.config.max_entries {
            return Err(Feature2dError::Capacity);
        }
        let padded_width = width
            .checked_add(self.config.padding.saturating_mul(2))
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        let padded_height = height
            .checked_add(self.config.padding.saturating_mul(2))
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        if padded_width > self.config.page_width || padded_height > self.config.page_height {
            return Err(Feature2dError::Capacity);
        }
        let mut selected = None;
        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some(rect) = allocate_in_page(page, &self.config, padded_width, padded_height) {
                selected = Some((index, rect));
                break;
            }
        }
        if selected.is_none() {
            if self.pages.len() >= self.config.max_pages as usize {
                return Err(Feature2dError::Capacity);
            }
            let id = AtlasPageId((self.pages.len() + 1) as u32);
            self.pages.push(AtlasPage {
                id,
                cursor_x: 0,
                cursor_y: 0,
                row_height: 0,
            });
            let index = self.pages.len() - 1;
            let rect = allocate_in_page(
                &mut self.pages[index],
                &self.config,
                padded_width,
                padded_height,
            )
            .ok_or(Feature2dError::Capacity)?;
            selected = Some((index, rect));
        }
        let (page_index, padded) = selected.unwrap();
        let entry = AtlasEntry {
            id,
            page: self.pages[page_index].id,
            rect: AtlasRect {
                x: padded.x + self.config.padding,
                y: padded.y + self.config.padding,
                width,
                height,
            },
            source_revision,
        };
        self.entries.insert(id, entry);
        Ok(entry)
    }

    pub fn hot_reload(
        &mut self,
        id: AtlasEntryId,
        source_revision: u64,
    ) -> Result<AtlasEntry, Feature2dError> {
        self.ensure_open()?;
        let entry = self.entries.get_mut(&id).ok_or(Feature2dError::Unknown)?;
        if source_revision <= entry.source_revision {
            return Err(Feature2dError::StaleRevision);
        }
        entry.source_revision = source_revision;
        Ok(*entry)
    }

    pub fn entry(&self, id: AtlasEntryId) -> Option<AtlasEntry> {
        self.entries.get(&id).copied()
    }

    fn ensure_open(&self) -> Result<(), Feature2dError> {
        if self.closed {
            Err(Feature2dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn allocate_in_page(
    page: &mut AtlasPage,
    config: &AtlasConfig,
    width: u32,
    height: u32,
) -> Option<AtlasRect> {
    if page.cursor_x.checked_add(width)? > config.page_width {
        page.cursor_x = 0;
        page.cursor_y = page.cursor_y.checked_add(page.row_height)?;
        page.row_height = 0;
    }
    if page.cursor_y.checked_add(height)? > config.page_height {
        return None;
    }
    let rect = AtlasRect {
        x: page.cursor_x,
        y: page.cursor_y,
        width,
        height,
    };
    page.cursor_x = page.cursor_x.checked_add(width)?;
    page.row_height = page.row_height.max(height);
    Some(rect)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SpriteId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedSprite {
    pub id: SpriteId,
    pub atlas: AtlasEntryId,
    pub position_milli: [i64; 2],
    pub size_milli: [u32; 2],
    pub pivot_milli: [i32; 2],
    pub tint: [u8; 4],
    pub layer_mask: u64,
    pub z_bucket: i32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Camera2d {
    pub center_milli: [i64; 2],
    pub half_extent_milli: [u64; 2],
    pub layer_mask: u64,
}

pub struct SpriteWorld {
    max_sprites: usize,
    sprites: BTreeMap<SpriteId, RetainedSprite>,
    revision: u64,
    closed: bool,
}

impl SpriteWorld {
    pub fn new(max_sprites: usize) -> Result<Self, Feature2dError> {
        if max_sprites == 0 {
            return Err(Feature2dError::InvalidConfig);
        }
        Ok(Self {
            max_sprites,
            sprites: BTreeMap::new(),
            revision: 0,
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> usize {
        let released = self.sprites.len();
        self.closed = true;
        self.sprites.clear();
        released
    }

    pub fn upsert(&mut self, sprite: RetainedSprite) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if sprite.id.0 == 0
            || sprite.atlas.0 == 0
            || sprite.size_milli.contains(&0)
            || sprite.layer_mask == 0
            || (!self.sprites.contains_key(&sprite.id) && self.sprites.len() >= self.max_sprites)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.sprites.insert(sprite.id, sprite);
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    pub fn remove(&mut self, id: SpriteId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.sprites.remove(&id).ok_or(Feature2dError::Unknown)?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    pub fn cull(&self, camera: Camera2d) -> Result<Vec<&RetainedSprite>, Feature2dError> {
        self.ensure_open()?;
        if camera.half_extent_milli.contains(&0) || camera.layer_mask == 0 {
            return Err(Feature2dError::InvalidDescriptor);
        }
        let mut visible = self
            .sprites
            .values()
            .filter(|sprite| {
                sprite.visible
                    && sprite.layer_mask & camera.layer_mask != 0
                    && sprite_visible(sprite, camera)
            })
            .collect::<Vec<_>>();
        visible.sort_by_key(|sprite| (sprite.z_bucket, sprite.id));
        Ok(visible)
    }

    fn ensure_open(&self) -> Result<(), Feature2dError> {
        if self.closed {
            Err(Feature2dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn sprite_visible(sprite: &RetainedSprite, camera: Camera2d) -> bool {
    let sprite_half = [
        i64::from(sprite.size_milli[0] / 2),
        i64::from(sprite.size_milli[1] / 2),
    ];
    (0..2).all(|axis| {
        let camera_half = i64::try_from(camera.half_extent_milli[axis]).unwrap_or(i64::MAX);
        (sprite.position_milli[axis] - camera.center_milli[axis]).abs()
            <= sprite_half[axis].saturating_add(camera_half)
    })
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TileChunkId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileChunk {
    pub id: TileChunkId,
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    pub tile_size_milli: u32,
    pub tiles: Vec<u32>,
    pub dirty_rows: BTreeSet<u32>,
    pub revision: u64,
}

impl TileChunk {
    pub fn new(
        id: TileChunkId,
        origin: [i32; 2],
        width: u32,
        height: u32,
        tile_size_milli: u32,
        max_tiles: usize,
    ) -> Result<Self, Feature2dError> {
        let count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|count| *count != 0 && *count <= max_tiles)
            .ok_or(Feature2dError::Capacity)?;
        if id.0 == 0 || tile_size_milli == 0 {
            return Err(Feature2dError::InvalidDescriptor);
        }
        Ok(Self {
            id,
            origin,
            width,
            height,
            tile_size_milli,
            tiles: vec![0; count],
            dirty_rows: BTreeSet::new(),
            revision: 0,
        })
    }

    pub fn set(&mut self, x: u32, y: u32, tile: u32) -> Result<u64, Feature2dError> {
        if x >= self.width || y >= self.height {
            return Err(Feature2dError::InvalidDescriptor);
        }
        let index = y as usize * self.width as usize + x as usize;
        if self.tiles[index] == tile {
            return Ok(self.revision);
        }
        self.tiles[index] = tile;
        self.dirty_rows.insert(y);
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    pub fn take_dirty_rows(&mut self) -> BTreeSet<u32> {
        std::mem::take(&mut self.dirty_rows)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Shape2d {
    Rect {
        min_milli: [i64; 2],
        max_milli: [i64; 2],
        color: [u8; 4],
    },
    Circle {
        center_milli: [i64; 2],
        radius_milli: u32,
        color: [u8; 4],
    },
    Line {
        from_milli: [i64; 2],
        to_milli: [i64; 2],
        width_milli: u32,
        color: [u8; 4],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlyphInstance {
    pub glyph: AtlasEntryId,
    pub position_milli: [i64; 2],
    pub size_milli: [u32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleEmitter {
    pub id: u64,
    pub seed: u64,
    pub position_milli: [i64; 2],
    pub spawn_per_tick: u32,
    pub lifetime_ticks: u32,
    pub velocity_min: [i32; 2],
    pub velocity_max: [i32; 2],
    pub layer_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Particle {
    pub emitter: u64,
    pub serial: u64,
    pub age: u32,
    pub lifetime: u32,
    pub position_milli: [i64; 2],
    pub velocity_milli: [i32; 2],
}

pub struct ParticleSystem {
    emitters: BTreeMap<u64, ParticleEmitter>,
    particles: Vec<Particle>,
    max_emitters: usize,
    max_particles: usize,
    last_tick: u64,
    next_serial: u64,
    closed: bool,
}

impl ParticleSystem {
    pub fn new(max_emitters: usize, max_particles: usize) -> Result<Self, Feature2dError> {
        if max_emitters == 0 || max_particles == 0 {
            return Err(Feature2dError::InvalidConfig);
        }
        Ok(Self {
            emitters: BTreeMap::new(),
            particles: Vec::new(),
            max_emitters,
            max_particles,
            last_tick: 0,
            next_serial: 1,
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> (usize, usize) {
        let released = (self.emitters.len(), self.particles.len());
        self.closed = true;
        self.emitters.clear();
        self.particles.clear();
        released
    }

    pub fn upsert_emitter(&mut self, emitter: ParticleEmitter) -> Result<(), Feature2dError> {
        self.ensure_open()?;
        if emitter.id == 0
            || emitter.seed == 0
            || emitter.spawn_per_tick == 0
            || emitter.lifetime_ticks == 0
            || emitter.layer_mask == 0
            || emitter.velocity_min[0] > emitter.velocity_max[0]
            || emitter.velocity_min[1] > emitter.velocity_max[1]
            || (!self.emitters.contains_key(&emitter.id)
                && self.emitters.len() >= self.max_emitters)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.emitters.insert(emitter.id, emitter);
        Ok(())
    }

    pub fn tick(&mut self, tick: u64) -> Result<&[Particle], Feature2dError> {
        self.ensure_open()?;
        if tick != self.last_tick + 1 {
            return Err(Feature2dError::TickSequence);
        }
        self.particles.retain_mut(|particle| {
            particle.age += 1;
            if particle.age >= particle.lifetime {
                return false;
            }
            particle.position_milli[0] += i64::from(particle.velocity_milli[0]);
            particle.position_milli[1] += i64::from(particle.velocity_milli[1]);
            true
        });
        for emitter in self.emitters.values() {
            for ordinal in 0..emitter.spawn_per_tick {
                if self.particles.len() >= self.max_particles {
                    break;
                }
                let random = split_mix64(emitter.seed ^ tick.rotate_left(17) ^ u64::from(ordinal));
                let velocity = [
                    range_value(random, emitter.velocity_min[0], emitter.velocity_max[0]),
                    range_value(
                        random.rotate_left(31),
                        emitter.velocity_min[1],
                        emitter.velocity_max[1],
                    ),
                ];
                self.particles.push(Particle {
                    emitter: emitter.id,
                    serial: self.next_serial,
                    age: 0,
                    lifetime: emitter.lifetime_ticks,
                    position_milli: emitter.position_milli,
                    velocity_milli: velocity,
                });
                self.next_serial = self
                    .next_serial
                    .checked_add(1)
                    .ok_or(Feature2dError::ArithmeticOverflow)?;
            }
        }
        self.particles
            .sort_by_key(|particle| (particle.emitter, particle.serial));
        self.last_tick = tick;
        Ok(&self.particles)
    }

    fn ensure_open(&self) -> Result<(), Feature2dError> {
        if self.closed {
            Err(Feature2dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn split_mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn range_value(random: u64, minimum: i32, maximum: i32) -> i32 {
    let span = i64::from(maximum) - i64::from(minimum) + 1;
    i32::try_from(i64::from(minimum) + i64::try_from(random % span as u64).unwrap()).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ShapeId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedShape {
    pub id: ShapeId,
    pub shape: Shape2d,
    pub layer_mask: u64,
    pub z_bucket: i32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TextRunId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedTextRun {
    pub id: TextRunId,
    pub font: u64,
    pub utf8: Vec<u8>,
    pub glyphs: Vec<GlyphInstance>,
    pub bounds_milli: [i64; 4],
    pub layer_mask: u64,
    pub z_bucket: i32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PostEffect2dId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostEffect2d {
    pub id: PostEffect2dId,
    pub shader_family: u64,
    pub parameters: Vec<i32>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedScene2dConfig {
    pub max_sprites: usize,
    pub max_shapes: usize,
    pub max_text_runs: usize,
    pub max_text_bytes: usize,
    pub max_glyphs: usize,
    pub max_tile_chunks: usize,
    pub max_post_effects: usize,
    pub max_post_parameters: usize,
}

impl Default for RetainedScene2dConfig {
    fn default() -> Self {
        Self {
            max_sprites: 100_000,
            max_shapes: 100_000,
            max_text_runs: 65_536,
            max_text_bytes: 64 * 1024 * 1024,
            max_glyphs: 1_000_000,
            max_tile_chunks: 16_384,
            max_post_effects: 256,
            max_post_parameters: 4096,
        }
    }
}

pub struct View2dBatch<'a> {
    pub revision: u64,
    pub sprites: Vec<&'a RetainedSprite>,
    pub shapes: Vec<&'a RetainedShape>,
    pub text_runs: Vec<&'a RetainedTextRun>,
    pub tile_chunks: Vec<&'a TileChunk>,
    pub post_effects: Vec<&'a PostEffect2d>,
}

pub struct RetainedScene2d {
    config: RetainedScene2dConfig,
    sprites: BTreeMap<SpriteId, RetainedSprite>,
    shapes: BTreeMap<ShapeId, RetainedShape>,
    text_runs: BTreeMap<TextRunId, RetainedTextRun>,
    tile_chunks: BTreeMap<TileChunkId, TileChunk>,
    post_effects: BTreeMap<PostEffect2dId, PostEffect2d>,
    text_bytes: usize,
    glyphs: usize,
    revision: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedScene2dOwnerSnapshot {
    pub closed: bool,
    pub sprites: usize,
    pub shapes: usize,
    pub text_runs: usize,
    pub text_bytes: usize,
    pub glyphs: usize,
    pub tile_chunks: usize,
    pub tile_values: usize,
    pub post_effects: usize,
    pub post_parameters: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedScene2dShutdownReport {
    pub released: RetainedScene2dOwnerSnapshot,
}

impl RetainedScene2d {
    pub fn new(config: RetainedScene2dConfig) -> Result<Self, Feature2dError> {
        if config.max_sprites == 0
            || config.max_shapes == 0
            || config.max_text_runs == 0
            || config.max_text_bytes == 0
            || config.max_glyphs == 0
            || config.max_tile_chunks == 0
            || config.max_post_effects == 0
            || config.max_post_parameters == 0
        {
            return Err(Feature2dError::InvalidConfig);
        }
        Ok(Self {
            config,
            sprites: BTreeMap::new(),
            shapes: BTreeMap::new(),
            text_runs: BTreeMap::new(),
            tile_chunks: BTreeMap::new(),
            post_effects: BTreeMap::new(),
            text_bytes: 0,
            glyphs: 0,
            revision: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> RetainedScene2dOwnerSnapshot {
        RetainedScene2dOwnerSnapshot {
            closed: self.closed,
            sprites: self.sprites.len(),
            shapes: self.shapes.len(),
            text_runs: self.text_runs.len(),
            text_bytes: self.text_bytes,
            glyphs: self.glyphs,
            tile_chunks: self.tile_chunks.len(),
            tile_values: self
                .tile_chunks
                .values()
                .map(|chunk| chunk.tiles.len())
                .sum(),
            post_effects: self.post_effects.len(),
            post_parameters: self
                .post_effects
                .values()
                .map(|effect| effect.parameters.len())
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> RetainedScene2dShutdownReport {
        let released = self.owner_snapshot();
        self.closed = true;
        self.sprites.clear();
        self.shapes.clear();
        self.text_runs.clear();
        self.tile_chunks.clear();
        self.post_effects.clear();
        self.text_bytes = 0;
        self.glyphs = 0;
        RetainedScene2dShutdownReport { released }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn upsert_sprite(&mut self, sprite: RetainedSprite) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if sprite.id.0 == 0
            || sprite.atlas.0 == 0
            || sprite.size_milli.contains(&0)
            || sprite.layer_mask == 0
            || (!self.sprites.contains_key(&sprite.id)
                && self.sprites.len() == self.config.max_sprites)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.sprites.insert(sprite.id, sprite);
        self.bump_revision()
    }

    pub fn upsert_shape(&mut self, shape: RetainedShape) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if shape.id.0 == 0
            || shape.layer_mask == 0
            || shape_bounds(&shape.shape).is_none()
            || (!self.shapes.contains_key(&shape.id) && self.shapes.len() == self.config.max_shapes)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.shapes.insert(shape.id, shape);
        self.bump_revision()
    }

    pub fn upsert_text(&mut self, run: RetainedTextRun) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if run.id.0 == 0
            || run.font == 0
            || run.utf8.is_empty()
            || std::str::from_utf8(&run.utf8).is_err()
            || run.glyphs.is_empty()
            || run.layer_mask == 0
            || run.bounds_milli[2] <= 0
            || run.bounds_milli[3] <= 0
            || run
                .glyphs
                .iter()
                .any(|glyph| glyph.glyph.0 == 0 || glyph.size_milli.contains(&0))
            || (!self.text_runs.contains_key(&run.id)
                && self.text_runs.len() == self.config.max_text_runs)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        let previous = self.text_runs.get(&run.id);
        let next_text_bytes = self
            .text_bytes
            .checked_sub(previous.map_or(0, |previous| previous.utf8.len()))
            .and_then(|bytes| bytes.checked_add(run.utf8.len()))
            .filter(|bytes| *bytes <= self.config.max_text_bytes)
            .ok_or(Feature2dError::Capacity)?;
        let next_glyphs = self
            .glyphs
            .checked_sub(previous.map_or(0, |previous| previous.glyphs.len()))
            .and_then(|glyphs| glyphs.checked_add(run.glyphs.len()))
            .filter(|glyphs| *glyphs <= self.config.max_glyphs)
            .ok_or(Feature2dError::Capacity)?;
        self.text_bytes = next_text_bytes;
        self.glyphs = next_glyphs;
        self.text_runs.insert(run.id, run);
        self.bump_revision()
    }

    pub fn upsert_tile_chunk(&mut self, chunk: TileChunk) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if chunk.id.0 == 0
            || chunk.revision == 0
            || chunk.width == 0
            || chunk.height == 0
            || chunk.tile_size_milli == 0
            || usize::try_from(chunk.width).ok().and_then(|width| {
                usize::try_from(chunk.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            }) != Some(chunk.tiles.len())
            || self
                .tile_chunks
                .get(&chunk.id)
                .is_some_and(|current| current.revision >= chunk.revision)
            || (!self.tile_chunks.contains_key(&chunk.id)
                && self.tile_chunks.len() == self.config.max_tile_chunks)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.tile_chunks.insert(chunk.id, chunk);
        self.bump_revision()
    }

    pub fn upsert_post_effect(&mut self, effect: PostEffect2d) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        if effect.id.0 == 0
            || effect.shader_family == 0
            || effect.parameters.len() > self.config.max_post_parameters
            || (!self.post_effects.contains_key(&effect.id)
                && self.post_effects.len() == self.config.max_post_effects)
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        self.post_effects.insert(effect.id, effect);
        self.bump_revision()
    }

    pub fn remove_sprite(&mut self, id: SpriteId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.sprites.remove(&id).ok_or(Feature2dError::Unknown)?;
        self.bump_revision()
    }

    pub fn remove_shape(&mut self, id: ShapeId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.shapes.remove(&id).ok_or(Feature2dError::Unknown)?;
        self.bump_revision()
    }

    pub fn remove_text(&mut self, id: TextRunId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        let run = self.text_runs.remove(&id).ok_or(Feature2dError::Unknown)?;
        self.text_bytes -= run.utf8.len();
        self.glyphs -= run.glyphs.len();
        self.bump_revision()
    }

    pub fn remove_tile_chunk(&mut self, id: TileChunkId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.tile_chunks
            .remove(&id)
            .ok_or(Feature2dError::Unknown)?;
        self.bump_revision()
    }

    pub fn remove_post_effect(&mut self, id: PostEffect2dId) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.post_effects
            .remove(&id)
            .ok_or(Feature2dError::Unknown)?;
        self.bump_revision()
    }

    pub fn compose_view<'a>(
        &'a self,
        camera: Camera2d,
        post_effects: &[PostEffect2dId],
    ) -> Result<View2dBatch<'a>, Feature2dError> {
        self.ensure_open()?;
        if camera.half_extent_milli.contains(&0)
            || camera.layer_mask == 0
            || post_effects.len() > self.config.max_post_effects
            || post_effects.iter().copied().collect::<BTreeSet<_>>().len() != post_effects.len()
        {
            return Err(Feature2dError::InvalidDescriptor);
        }
        let mut sprites = self
            .sprites
            .values()
            .filter(|sprite| {
                sprite.visible
                    && sprite.layer_mask & camera.layer_mask != 0
                    && sprite_visible(sprite, camera)
            })
            .collect::<Vec<_>>();
        sprites.sort_by_key(|sprite| (sprite.z_bucket, sprite.id));
        let mut shapes = self
            .shapes
            .values()
            .filter(|shape| {
                shape.visible
                    && shape.layer_mask & camera.layer_mask != 0
                    && shape_bounds(&shape.shape)
                        .is_some_and(|bounds| bounds_visible(bounds, camera))
            })
            .collect::<Vec<_>>();
        shapes.sort_by_key(|shape| (shape.z_bucket, shape.id));
        let mut text_runs = self
            .text_runs
            .values()
            .filter(|run| {
                run.visible
                    && run.layer_mask & camera.layer_mask != 0
                    && bounds_visible(run.bounds_milli, camera)
            })
            .collect::<Vec<_>>();
        text_runs.sort_by_key(|run| (run.z_bucket, run.id));
        let tile_chunks = self
            .tile_chunks
            .values()
            .filter(|chunk| tile_chunk_visible(chunk, camera))
            .collect();
        let post_effects = post_effects
            .iter()
            .map(|id| {
                self.post_effects
                    .get(id)
                    .filter(|effect| effect.enabled)
                    .ok_or(Feature2dError::Unknown)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(View2dBatch {
            revision: self.revision,
            sprites,
            shapes,
            text_runs,
            tile_chunks,
            post_effects,
        })
    }

    fn bump_revision(&mut self) -> Result<u64, Feature2dError> {
        self.ensure_open()?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Feature2dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    fn ensure_open(&self) -> Result<(), Feature2dError> {
        if self.closed {
            Err(Feature2dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn shape_bounds(shape: &Shape2d) -> Option<[i64; 4]> {
    match shape {
        Shape2d::Rect {
            min_milli,
            max_milli,
            ..
        } if min_milli[0] < max_milli[0] && min_milli[1] < max_milli[1] => Some([
            min_milli[0],
            min_milli[1],
            max_milli[0] - min_milli[0],
            max_milli[1] - min_milli[1],
        ]),
        Shape2d::Circle {
            center_milli,
            radius_milli,
            ..
        } if *radius_milli != 0 => {
            let radius = i64::from(*radius_milli);
            Some([
                center_milli[0] - radius,
                center_milli[1] - radius,
                radius * 2,
                radius * 2,
            ])
        }
        Shape2d::Line {
            from_milli,
            to_milli,
            width_milli,
            ..
        } if *width_milli != 0 => {
            let half = (i64::from(*width_milli) + 1) / 2;
            let min_x = from_milli[0].min(to_milli[0]) - half;
            let min_y = from_milli[1].min(to_milli[1]) - half;
            Some([
                min_x,
                min_y,
                from_milli[0].max(to_milli[0]) + half - min_x,
                from_milli[1].max(to_milli[1]) + half - min_y,
            ])
        }
        _ => None,
    }
}

fn bounds_visible(bounds: [i64; 4], camera: Camera2d) -> bool {
    if bounds[2] <= 0 || bounds[3] <= 0 {
        return false;
    }
    let camera_half_x = i64::try_from(camera.half_extent_milli[0]).unwrap_or(i64::MAX);
    let camera_half_y = i64::try_from(camera.half_extent_milli[1]).unwrap_or(i64::MAX);
    bounds[0] <= camera.center_milli[0].saturating_add(camera_half_x)
        && bounds[0].saturating_add(bounds[2])
            >= camera.center_milli[0].saturating_sub(camera_half_x)
        && bounds[1] <= camera.center_milli[1].saturating_add(camera_half_y)
        && bounds[1].saturating_add(bounds[3])
            >= camera.center_milli[1].saturating_sub(camera_half_y)
}

fn tile_chunk_visible(chunk: &TileChunk, camera: Camera2d) -> bool {
    let width = i64::from(chunk.width).saturating_mul(i64::from(chunk.tile_size_milli));
    let height = i64::from(chunk.height).saturating_mul(i64::from(chunk.tile_size_milli));
    bounds_visible(
        [
            i64::from(chunk.origin[0]).saturating_mul(i64::from(chunk.tile_size_milli)),
            i64::from(chunk.origin[1]).saturating_mul(i64::from(chunk.tile_size_milli)),
            width,
            height,
        ],
        camera,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite(id: u64, position_milli: [i64; 2], z_bucket: i32) -> RetainedSprite {
        RetainedSprite {
            id: SpriteId(id),
            atlas: AtlasEntryId(id),
            position_milli,
            size_milli: [1_000, 1_000],
            pivot_milli: [500, 500],
            tint: [255; 4],
            layer_mask: 1,
            z_bucket,
            visible: true,
        }
    }

    #[test]
    fn atlas_reload_preserves_location_and_rejects_stale_revision() {
        let mut atlas = AtlasAllocator::new(AtlasConfig {
            page_width: 64,
            page_height: 64,
            max_pages: 2,
            max_entries: 8,
            padding: 1,
        })
        .unwrap();
        let allocated = atlas.allocate(AtlasEntryId(7), 16, 12, 4).unwrap();
        let reloaded = atlas.hot_reload(AtlasEntryId(7), 5).unwrap();

        assert_eq!(reloaded.page, allocated.page);
        assert_eq!(reloaded.rect, allocated.rect);
        assert_eq!(reloaded.source_revision, 5);
        assert_eq!(
            atlas.hot_reload(AtlasEntryId(7), 5),
            Err(Feature2dError::StaleRevision)
        );
    }

    #[test]
    fn camera_composition_reads_retained_scene_without_changing_revision() {
        let mut scene = RetainedScene2d::new(RetainedScene2dConfig::default()).unwrap();
        scene.upsert_sprite(sprite(1, [0, 0], 2)).unwrap();
        scene.upsert_sprite(sprite(2, [500, 0], 1)).unwrap();
        scene.upsert_sprite(sprite(3, [50_000, 0], 0)).unwrap();
        let retained_revision = scene.revision();

        let first = scene
            .compose_view(
                Camera2d {
                    center_milli: [0, 0],
                    half_extent_milli: [2_000, 2_000],
                    layer_mask: 1,
                },
                &[],
            )
            .unwrap();
        assert_eq!(
            first
                .sprites
                .iter()
                .map(|sprite| sprite.id)
                .collect::<Vec<_>>(),
            vec![SpriteId(2), SpriteId(1)]
        );
        assert_eq!(first.revision, retained_revision);

        let moved = scene
            .compose_view(
                Camera2d {
                    center_milli: [50_000, 0],
                    half_extent_milli: [2_000, 2_000],
                    layer_mask: 1,
                },
                &[],
            )
            .unwrap();
        assert_eq!(
            moved
                .sprites
                .iter()
                .map(|sprite| sprite.id)
                .collect::<Vec<_>>(),
            vec![SpriteId(3)]
        );
        assert_eq!(scene.revision(), retained_revision);
    }

    #[test]
    fn particle_tick_is_deterministic_and_sequence_checked() {
        let emitter = ParticleEmitter {
            id: 1,
            seed: 9,
            position_milli: [10, 20],
            spawn_per_tick: 3,
            lifetime_ticks: 4,
            velocity_min: [-2, -3],
            velocity_max: [2, 3],
            layer_mask: 1,
        };
        let mut left = ParticleSystem::new(2, 32).unwrap();
        let mut right = ParticleSystem::new(2, 32).unwrap();
        left.upsert_emitter(emitter).unwrap();
        right.upsert_emitter(emitter).unwrap();

        for tick in 1..=8 {
            assert_eq!(left.tick(tick).unwrap(), right.tick(tick).unwrap());
        }
        assert_eq!(left.tick(10), Err(Feature2dError::TickSequence));
    }
}
