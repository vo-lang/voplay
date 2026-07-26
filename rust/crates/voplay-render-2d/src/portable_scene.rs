use std::collections::BTreeMap;

use crate::{
    decode_particle_emitter_2d, decode_shape_2d, decode_sprite_2d, decode_text_2d,
    decode_tile_chunk_2d, decode_view_2d, AtlasEntryId, DrawCommand, Feature2dError,
    RetainedSprite, RetainedTextRun, SpriteId, TextRunId, Wire2dError, WireParticleEmitter2d,
    WireShape2d, WireShapeKind2d, WireSprite2d, WireText2d, WireTileCell2d, WireTileChunk2d,
    WireView2d,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableScene2dConfig {
    pub max_sprites: usize,
    pub max_shapes: usize,
    pub max_text_runs: usize,
    pub max_text_bytes: usize,
    pub max_tile_chunks: usize,
    pub max_tile_cells: usize,
    pub max_particle_emitters: usize,
    pub max_particles: usize,
    pub max_path_values: usize,
    pub max_views: usize,
    pub max_post_effects_per_view: usize,
}

impl Default for PortableScene2dConfig {
    fn default() -> Self {
        Self {
            max_sprites: 100_000,
            max_shapes: 100_000,
            max_text_runs: 65_536,
            max_text_bytes: 64 * 1024 * 1024,
            max_tile_chunks: 16_384,
            max_tile_cells: 1_048_576,
            max_particle_emitters: 16_384,
            max_particles: 1_000_000,
            max_path_values: 131_072,
            max_views: 256,
            max_post_effects_per_view: 256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableTileChunk2d {
    pub id: u64,
    pub revision: u64,
    pub origin_milli: [i64; 2],
    pub cell_size_milli: [u32; 2],
    pub width: u32,
    pub height: u32,
    pub cells: BTreeMap<u64, WireTileCell2d>,
    pub layers: u64,
    pub z: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableParticle2d {
    pub emitter: u64,
    pub serial: u64,
    pub age: u32,
    pub lifetime: u32,
    pub image: u64,
    pub position_milli: [i64; 2],
    pub velocity_milli: [i64; 2],
    pub color: [u8; 4],
    pub size_milli: u32,
    pub layers: u64,
    pub z: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortableScene2dError {
    Closed,
    InvalidConfig,
    InvalidDescriptor,
    Capacity,
    StaleRevision,
    ArithmeticOverflow,
    Decode(Wire2dError),
    Shaping,
}

impl From<Wire2dError> for PortableScene2dError {
    fn from(value: Wire2dError) -> Self {
        Self::Decode(value)
    }
}

impl From<Feature2dError> for PortableScene2dError {
    fn from(value: Feature2dError) -> Self {
        match value {
            Feature2dError::Closed => Self::Closed,
            Feature2dError::Capacity => Self::Capacity,
            Feature2dError::StaleRevision => Self::StaleRevision,
            Feature2dError::ArithmeticOverflow => Self::ArithmeticOverflow,
            _ => Self::InvalidDescriptor,
        }
    }
}

pub trait TextShaper2d {
    fn shutdown(&mut self) {}

    fn shape(&mut self, source: &WireText2d) -> Result<RetainedTextRun, PortableScene2dError>;
}

pub trait PortableDrawResolver2d {
    fn shutdown(&mut self) {}

    fn text_commands(
        &mut self,
        source: &WireText2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError>;

    fn tile_commands(
        &mut self,
        source: &PortableTileChunk2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError>;

    fn particle_commands(
        &mut self,
        source: &PortableParticle2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReferenceDrawResolver2d;

impl PortableDrawResolver2d for ReferenceDrawResolver2d {
    fn text_commands(
        &mut self,
        source: &WireText2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        let glyph_width = u32::try_from(
            u64::from(source.size_milli)
                .checked_mul(3)
                .ok_or(PortableScene2dError::ArithmeticOverflow)?
                / 5,
        )
        .map_err(|_| PortableScene2dError::ArithmeticOverflow)?
        .max(1);
        let advance = glyph_width.saturating_add((source.size_milli / 8).max(1));
        let line_advance = source
            .size_milli
            .saturating_add((source.size_milli / 4).max(1));
        let mut cursor = source.position_milli;
        let line_start = source.position_milli[0];
        let mut commands = Vec::new();
        for character in std::str::from_utf8(&source.text)
            .map_err(|_| PortableScene2dError::InvalidDescriptor)?
            .chars()
        {
            if character == '\n'
                || (source.max_width_milli != 0
                    && cursor[0]
                        .saturating_sub(line_start)
                        .saturating_add(i64::from(glyph_width))
                        > i64::from(source.max_width_milli))
            {
                cursor[0] = line_start;
                cursor[1] = cursor[1].saturating_add(i64::from(line_advance));
                if character == '\n' {
                    continue;
                }
            }
            if !character.is_whitespace() {
                if let Some(command) = solid_command(
                    [
                        cursor[0],
                        cursor[1],
                        i64::from(glyph_width),
                        i64::from(source.size_milli),
                    ],
                    view,
                    source.color,
                ) {
                    commands.push(command);
                }
            }
            cursor[0] = cursor[0].saturating_add(i64::from(advance));
        }
        Ok(commands)
    }

    fn tile_commands(
        &mut self,
        source: &PortableTileChunk2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        Ok(source
            .cells
            .values()
            .filter_map(|cell| {
                solid_command(
                    [
                        source.origin_milli[0].saturating_add(
                            i64::from(cell.x).saturating_mul(i64::from(source.cell_size_milli[0])),
                        ),
                        source.origin_milli[1].saturating_add(
                            i64::from(cell.y).saturating_mul(i64::from(source.cell_size_milli[1])),
                        ),
                        i64::from(source.cell_size_milli[0]),
                        i64::from(source.cell_size_milli[1]),
                    ],
                    view,
                    cell.tint,
                )
            })
            .collect())
    }

    fn particle_commands(
        &mut self,
        source: &PortableParticle2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        Ok(solid_command(particle_bounds(source), view, source.color)
            .into_iter()
            .collect())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureRegion2d {
    pub texture: u64,
    pub source: [u32; 4],
}

#[derive(Clone)]
pub struct AtlasDrawResolver2d<S> {
    shaper: S,
    regions: BTreeMap<u64, TextureRegion2d>,
}

impl<S> AtlasDrawResolver2d<S> {
    pub fn new(shaper: S) -> Self {
        Self {
            shaper,
            regions: BTreeMap::new(),
        }
    }

    pub fn upsert_region(
        &mut self,
        asset: u64,
        region: TextureRegion2d,
    ) -> Result<(), PortableScene2dError> {
        if asset == 0 || region.texture == 0 || region.source[2] == 0 || region.source[3] == 0 {
            return Err(PortableScene2dError::InvalidDescriptor);
        }
        self.regions.insert(asset, region);
        Ok(())
    }

    pub fn remove_region(&mut self, asset: u64) -> bool {
        self.regions.remove(&asset).is_some()
    }
}

impl<S: TextShaper2d> PortableDrawResolver2d for AtlasDrawResolver2d<S> {
    fn shutdown(&mut self) {
        self.regions.clear();
        self.shaper.shutdown();
    }

    fn text_commands(
        &mut self,
        source: &WireText2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        let shaped = self.shaper.shape(source)?;
        if shaped.id != TextRunId(source.id)
            || shaped.font != source.font
            || shaped.utf8 != source.text
            || shaped.layer_mask != source.layers
            || shaped.z_bucket != clamp_z_bucket(source.z)
        {
            return Err(PortableScene2dError::Shaping);
        }
        let mut commands = Vec::with_capacity(shaped.glyphs.len());
        for glyph in &shaped.glyphs {
            let region = self
                .regions
                .get(&glyph.glyph.0)
                .ok_or(PortableScene2dError::InvalidDescriptor)?;
            if let Some(command) = textured_command(
                [
                    glyph.position_milli[0],
                    glyph.position_milli[1],
                    i64::from(glyph.size_milli[0]),
                    i64::from(glyph.size_milli[1]),
                ],
                view,
                *region,
                glyph.color,
            ) {
                commands.push(command);
            }
        }
        Ok(commands)
    }

    fn tile_commands(
        &mut self,
        source: &PortableTileChunk2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        let mut commands = Vec::with_capacity(source.cells.len());
        for cell in source.cells.values() {
            if cell.flags != 0 {
                return Err(PortableScene2dError::InvalidDescriptor);
            }
            let region = self
                .regions
                .get(&u64::from(cell.tile))
                .ok_or(PortableScene2dError::InvalidDescriptor)?;
            let bounds = [
                source.origin_milli[0].saturating_add(
                    i64::from(cell.x).saturating_mul(i64::from(source.cell_size_milli[0])),
                ),
                source.origin_milli[1].saturating_add(
                    i64::from(cell.y).saturating_mul(i64::from(source.cell_size_milli[1])),
                ),
                i64::from(source.cell_size_milli[0]),
                i64::from(source.cell_size_milli[1]),
            ];
            if let Some(command) = textured_command(bounds, view, *region, cell.tint) {
                commands.push(command);
            }
        }
        Ok(commands)
    }

    fn particle_commands(
        &mut self,
        source: &PortableParticle2d,
        view: &WireView2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        let region = self
            .regions
            .get(&source.image)
            .ok_or(PortableScene2dError::InvalidDescriptor)?;
        let Some(command) = textured_command(particle_bounds(source), view, *region, source.color)
        else {
            return Ok(Vec::new());
        };
        Ok(vec![command])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortableDraw2d<'a> {
    Sprite(&'a WireSprite2d),
    Shape(&'a WireShape2d),
    Text(&'a WireText2d),
    TileChunk(&'a PortableTileChunk2d),
    Particle(&'a PortableParticle2d),
}

pub struct PortableView2dBatch<'a> {
    pub scene_revision: u64,
    pub view: &'a WireView2d,
    pub draws: Vec<PortableDraw2d<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableScene2dOwnerSnapshot {
    pub closed: bool,
    pub sprites: usize,
    pub shapes: usize,
    pub text_runs: usize,
    pub text_bytes: usize,
    pub tile_chunks: usize,
    pub particle_emitters: usize,
    pub particles: usize,
    pub views: usize,
}

#[derive(Clone)]
pub struct PortableScene2d {
    config: PortableScene2dConfig,
    sprites: BTreeMap<u64, WireSprite2d>,
    shapes: BTreeMap<u64, WireShape2d>,
    text_runs: BTreeMap<u64, WireText2d>,
    tile_chunks: BTreeMap<u64, PortableTileChunk2d>,
    particle_emitters: BTreeMap<u64, WireParticleEmitter2d>,
    particles: Vec<PortableParticle2d>,
    views: BTreeMap<u64, WireView2d>,
    text_bytes: usize,
    last_particle_tick: u64,
    next_particle_serial: u64,
    revision: u64,
    closed: bool,
}

impl PortableScene2d {
    pub fn new(config: PortableScene2dConfig) -> Result<Self, PortableScene2dError> {
        if config.max_sprites == 0
            || config.max_shapes == 0
            || config.max_text_runs == 0
            || config.max_text_bytes == 0
            || config.max_tile_chunks == 0
            || config.max_tile_cells == 0
            || config.max_particle_emitters == 0
            || config.max_particles == 0
            || config.max_path_values == 0
            || config.max_views == 0
            || config.max_post_effects_per_view == 0
        {
            return Err(PortableScene2dError::InvalidConfig);
        }
        Ok(Self {
            config,
            sprites: BTreeMap::new(),
            shapes: BTreeMap::new(),
            text_runs: BTreeMap::new(),
            tile_chunks: BTreeMap::new(),
            particle_emitters: BTreeMap::new(),
            particles: Vec::new(),
            views: BTreeMap::new(),
            text_bytes: 0,
            last_particle_tick: 0,
            next_particle_serial: 1,
            revision: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> PortableScene2dOwnerSnapshot {
        PortableScene2dOwnerSnapshot {
            closed: self.closed,
            sprites: self.sprites.len(),
            shapes: self.shapes.len(),
            text_runs: self.text_runs.len(),
            text_bytes: self.text_bytes,
            tile_chunks: self.tile_chunks.len(),
            particle_emitters: self.particle_emitters.len(),
            particles: self.particles.len(),
            views: self.views.len(),
        }
    }

    pub fn shutdown(&mut self) -> PortableScene2dOwnerSnapshot {
        let released = self.owner_snapshot();
        if self.closed {
            return released;
        }
        self.closed = true;
        self.sprites.clear();
        self.shapes.clear();
        self.text_runs.clear();
        self.text_bytes = 0;
        self.tile_chunks.clear();
        self.particle_emitters.clear();
        self.particles.clear();
        self.views.clear();
        released
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn sprite(&self, id: u64) -> Option<&WireSprite2d> {
        self.sprites.get(&id)
    }

    pub fn shape(&self, id: u64) -> Option<&WireShape2d> {
        self.shapes.get(&id)
    }

    pub fn text(&self, id: u64) -> Option<&WireText2d> {
        self.text_runs.get(&id)
    }

    pub fn tile_chunk(&self, id: u64) -> Option<&PortableTileChunk2d> {
        self.tile_chunks.get(&id)
    }

    pub fn particle_emitter(&self, id: u64) -> Option<&WireParticleEmitter2d> {
        self.particle_emitters.get(&id)
    }

    pub fn particles(&self) -> &[PortableParticle2d] {
        &self.particles
    }

    pub fn view(&self, id: u64) -> Option<&WireView2d> {
        self.views.get(&id)
    }

    pub fn apply_view(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let view = decode_view_2d(bytes, self.config.max_post_effects_per_view)?;
        ensure_capacity(
            self.views.contains_key(&view.id),
            self.views.len(),
            self.config.max_views,
        )?;
        self.views.insert(view.id, view);
        self.bump_revision()
    }

    pub fn apply_sprite(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let sprite = decode_sprite_2d(bytes)?;
        ensure_capacity(
            self.sprites.contains_key(&sprite.id),
            self.sprites.len(),
            self.config.max_sprites,
        )?;
        self.sprites.insert(sprite.id, sprite);
        self.bump_revision()
    }

    pub fn apply_shape(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let shape = decode_shape_2d(bytes, self.config.max_path_values)?;
        ensure_capacity(
            self.shapes.contains_key(&shape.id),
            self.shapes.len(),
            self.config.max_shapes,
        )?;
        self.shapes.insert(shape.id, shape);
        self.bump_revision()
    }

    pub fn apply_text(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let run = decode_text_2d(bytes, self.config.max_text_bytes)?;
        ensure_capacity(
            self.text_runs.contains_key(&run.id),
            self.text_runs.len(),
            self.config.max_text_runs,
        )?;
        let previous_length = self.text_runs.get(&run.id).map_or(0, |run| run.text.len());
        let next_text_bytes = self
            .text_bytes
            .checked_sub(previous_length)
            .and_then(|length| length.checked_add(run.text.len()))
            .filter(|length| *length <= self.config.max_text_bytes)
            .ok_or(PortableScene2dError::Capacity)?;
        self.text_bytes = next_text_bytes;
        self.text_runs.insert(run.id, run);
        self.bump_revision()
    }

    pub fn apply_tile_chunk(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let update = decode_tile_chunk_2d(
            bytes,
            self.config.max_tile_cells,
            self.config.max_tile_cells,
        )?;
        let cell_count = tile_cell_count(update.width, update.height)?;
        if update
            .removed
            .iter()
            .any(|cell| usize::try_from(*cell).map_or(true, |cell| cell >= cell_count))
        {
            return Err(PortableScene2dError::InvalidDescriptor);
        }
        let mut candidate = match self.tile_chunks.get(&update.chunk) {
            Some(current) => {
                if update.revision <= current.revision
                    || update.width != current.width
                    || update.height != current.height
                    || update.origin_milli != current.origin_milli
                    || update.cell_size_milli != current.cell_size_milli
                {
                    return Err(PortableScene2dError::StaleRevision);
                }
                current.clone()
            }
            None => {
                ensure_capacity(false, self.tile_chunks.len(), self.config.max_tile_chunks)?;
                PortableTileChunk2d {
                    id: update.chunk,
                    revision: 0,
                    origin_milli: update.origin_milli,
                    cell_size_milli: update.cell_size_milli,
                    width: update.width,
                    height: update.height,
                    cells: BTreeMap::new(),
                    layers: update.layers,
                    z: update.z,
                }
            }
        };
        for removed in &update.removed {
            candidate.cells.remove(removed);
        }
        for cell in update.cells {
            candidate
                .cells
                .insert(tile_cell_key(cell.x, cell.y, update.width)?, cell);
        }
        if candidate.cells.len() > self.config.max_tile_cells {
            return Err(PortableScene2dError::Capacity);
        }
        candidate.revision = update.revision;
        candidate.layers = update.layers;
        candidate.z = update.z;
        self.tile_chunks.insert(update.chunk, candidate);
        self.bump_revision()
    }

    pub fn apply_particle_emitter(&mut self, bytes: &[u8]) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let emitter = decode_particle_emitter_2d(bytes)?;
        ensure_capacity(
            self.particle_emitters.contains_key(&emitter.id),
            self.particle_emitters.len(),
            self.config.max_particle_emitters,
        )?;
        self.particle_emitters.insert(emitter.id, emitter);
        self.particles
            .retain(|particle| particle.emitter != emitter.id);
        self.bump_revision()
    }

    pub fn remove_sprite(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        remove(&mut self.sprites, id)?;
        self.bump_revision()
    }

    pub fn remove_shape(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        remove(&mut self.shapes, id)?;
        self.bump_revision()
    }

    pub fn remove_text(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        let run = self
            .text_runs
            .remove(&id)
            .ok_or(PortableScene2dError::InvalidDescriptor)?;
        self.text_bytes -= run.text.len();
        self.bump_revision()
    }

    pub fn remove_tile_chunk(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        remove(&mut self.tile_chunks, id)?;
        self.bump_revision()
    }

    pub fn remove_particle_emitter(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        remove(&mut self.particle_emitters, id)?;
        self.particles.retain(|particle| particle.emitter != id);
        self.bump_revision()
    }

    pub fn remove_view(&mut self, id: u64) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        remove(&mut self.views, id)?;
        self.bump_revision()
    }

    pub fn retained_sprite(&self, id: u64) -> Option<RetainedSprite> {
        let sprite = self.sprites.get(&id)?;
        Some(RetainedSprite {
            id: SpriteId(sprite.id),
            atlas: AtlasEntryId(sprite.image),
            position_milli: sprite.position_milli,
            size_milli: sprite.size_milli,
            pivot_milli: sprite.anchor_milli,
            tint: sprite.color,
            layer_mask: sprite.layers,
            z_bucket: clamp_z_bucket(sprite.z),
            visible: sprite.visible,
        })
    }

    pub fn shape_text(
        &self,
        id: u64,
        shaper: &mut impl TextShaper2d,
    ) -> Result<RetainedTextRun, PortableScene2dError> {
        self.ensure_open()?;
        let source = self
            .text_runs
            .get(&id)
            .ok_or(PortableScene2dError::InvalidDescriptor)?;
        let shaped = shaper.shape(source)?;
        if shaped.id != TextRunId(source.id)
            || shaped.font != source.font
            || shaped.utf8 != source.text
            || shaped.layer_mask != source.layers
            || shaped.z_bucket != clamp_z_bucket(source.z)
        {
            return Err(PortableScene2dError::Shaping);
        }
        Ok(shaped)
    }

    pub fn compose_view(&self, id: u64) -> Result<PortableView2dBatch<'_>, PortableScene2dError> {
        self.ensure_open()?;
        let view = self
            .views
            .get(&id)
            .ok_or(PortableScene2dError::InvalidDescriptor)?;
        let bounds = view_world_bounds(view)?;
        let mut draws = Vec::new();
        draws.extend(
            self.sprites
                .values()
                .filter(|sprite| {
                    sprite.visible
                        && sprite.layers & view.layers != 0
                        && intersects(sprite_bounds(sprite), bounds)
                })
                .map(PortableDraw2d::Sprite),
        );
        draws.extend(
            self.shapes
                .values()
                .filter(|shape| {
                    shape.layers & view.layers != 0 && intersects(shape.bounds_milli, bounds)
                })
                .map(PortableDraw2d::Shape),
        );
        draws.extend(
            self.text_runs
                .values()
                .filter(|text| {
                    text.layers & view.layers != 0 && intersects(text_bounds(text), bounds)
                })
                .map(PortableDraw2d::Text),
        );
        draws.extend(
            self.tile_chunks
                .values()
                .filter(|chunk| {
                    chunk.layers & view.layers != 0 && intersects(tile_chunk_bounds(chunk), bounds)
                })
                .map(PortableDraw2d::TileChunk),
        );
        draws.extend(
            self.particles
                .iter()
                .filter(|particle| {
                    particle.layers & view.layers != 0
                        && intersects(particle_bounds(particle), bounds)
                })
                .map(PortableDraw2d::Particle),
        );
        draws.sort_by_key(|draw| (draw_z(*draw), draw_id(*draw)));
        Ok(PortableView2dBatch {
            scene_revision: self.revision,
            view,
            draws,
        })
    }

    pub fn portable_draw_commands(
        &self,
        id: u64,
        max_commands: usize,
        resolver: &mut impl PortableDrawResolver2d,
    ) -> Result<Vec<DrawCommand>, PortableScene2dError> {
        if max_commands == 0 {
            return Err(PortableScene2dError::InvalidConfig);
        }
        let batch = self.compose_view(id)?;
        let mut commands = vec![DrawCommand::Clear {
            color: batch.view.clear_color,
        }];
        for draw in batch.draws {
            match draw {
                PortableDraw2d::Sprite(sprite) => {
                    let source_width = if sprite.atlas.width == 0 {
                        sprite.size_milli[0]
                    } else {
                        sprite.atlas.width
                    };
                    let source_height = if sprite.atlas.height == 0 {
                        sprite.size_milli[1]
                    } else {
                        sprite.atlas.height
                    };
                    if let Some(command) = textured_command(
                        sprite_bounds(sprite),
                        batch.view,
                        TextureRegion2d {
                            texture: sprite.image,
                            source: [sprite.atlas.x, sprite.atlas.y, source_width, source_height],
                        },
                        sprite.color,
                    ) {
                        commands.push(command);
                    }
                }
                PortableDraw2d::Shape(shape) => {
                    tessellate_shape(shape, batch.view, &mut commands)?;
                }
                PortableDraw2d::Text(text) => {
                    commands.extend(resolver.text_commands(text, batch.view)?);
                }
                PortableDraw2d::TileChunk(chunk) => {
                    commands.extend(resolver.tile_commands(chunk, batch.view)?);
                }
                PortableDraw2d::Particle(particle) => {
                    commands.extend(resolver.particle_commands(particle, batch.view)?);
                }
            }
            if commands.len() > max_commands {
                return Err(PortableScene2dError::Capacity);
            }
        }
        Ok(commands)
    }

    pub fn tick_particles(
        &mut self,
        tick: u64,
    ) -> Result<&[PortableParticle2d], PortableScene2dError> {
        self.ensure_open()?;
        if self.last_particle_tick.checked_add(1) != Some(tick) {
            return Err(PortableScene2dError::InvalidDescriptor);
        }
        let mut particles = Vec::with_capacity(
            self.config.max_particles.min(
                self.particles
                    .len()
                    .saturating_add(self.particle_emitters.len()),
            ),
        );
        for particle in &self.particles {
            let age = particle
                .age
                .checked_add(1)
                .ok_or(PortableScene2dError::ArithmeticOverflow)?;
            if age >= particle.lifetime {
                continue;
            }
            let emitter = self
                .particle_emitters
                .get(&particle.emitter)
                .ok_or(PortableScene2dError::InvalidDescriptor)?;
            let mut next = *particle;
            next.age = age;
            for axis in 0..2 {
                next.position_milli[axis] = next.position_milli[axis]
                    .checked_add(next.velocity_milli[axis])
                    .ok_or(PortableScene2dError::ArithmeticOverflow)?;
            }
            next.color = interpolate_color(
                emitter.start_color,
                emitter.end_color,
                age,
                particle.lifetime,
            );
            next.size_milli = interpolate_size(
                emitter.start_size_milli,
                emitter.end_size_milli,
                age,
                particle.lifetime,
            );
            particles.push(next);
        }
        let mut next_serial = self.next_particle_serial;
        for emitter in self.particle_emitters.values() {
            let alive = particles
                .iter()
                .filter(|particle| particle.emitter == emitter.id)
                .count();
            let available = usize::try_from(emitter.max_particles)
                .unwrap_or(usize::MAX)
                .saturating_sub(alive);
            let spawn = usize::try_from(emitter.spawn_per_tick)
                .unwrap_or(usize::MAX)
                .min(available);
            for ordinal in 0..spawn {
                if particles.len() == self.config.max_particles {
                    break;
                }
                let random = split_mix64_2d(emitter.seed ^ tick.rotate_left(17) ^ ordinal as u64);
                let velocity_milli = [
                    random_i64_2d(
                        random,
                        emitter.velocity_min_milli[0],
                        emitter.velocity_max_milli[0],
                    )?,
                    random_i64_2d(
                        random.rotate_left(31),
                        emitter.velocity_min_milli[1],
                        emitter.velocity_max_milli[1],
                    )?,
                ];
                particles.push(PortableParticle2d {
                    emitter: emitter.id,
                    serial: next_serial,
                    age: 0,
                    lifetime: emitter.lifetime_ticks,
                    image: emitter.image,
                    position_milli: emitter.position_milli,
                    velocity_milli,
                    color: emitter.start_color,
                    size_milli: emitter.start_size_milli,
                    layers: emitter.layers,
                    z: emitter.z,
                });
                next_serial = next_serial
                    .checked_add(1)
                    .ok_or(PortableScene2dError::ArithmeticOverflow)?;
            }
        }
        particles.sort_by_key(|particle| (particle.z, particle.emitter, particle.serial));
        self.particles = particles;
        self.last_particle_tick = tick;
        self.next_particle_serial = next_serial;
        Ok(&self.particles)
    }

    fn bump_revision(&mut self) -> Result<u64, PortableScene2dError> {
        self.ensure_open()?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(PortableScene2dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    fn ensure_open(&self) -> Result<(), PortableScene2dError> {
        if self.closed {
            Err(PortableScene2dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn ensure_capacity(
    replacing: bool,
    length: usize,
    maximum: usize,
) -> Result<(), PortableScene2dError> {
    if !replacing && length >= maximum {
        Err(PortableScene2dError::Capacity)
    } else {
        Ok(())
    }
}

fn remove<T>(values: &mut BTreeMap<u64, T>, id: u64) -> Result<(), PortableScene2dError> {
    values
        .remove(&id)
        .map(|_| ())
        .ok_or(PortableScene2dError::InvalidDescriptor)
}

fn tile_cell_count(width: u32, height: u32) -> Result<usize, PortableScene2dError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(PortableScene2dError::Capacity)
}

fn tile_cell_key(x: u32, y: u32, width: u32) -> Result<u64, PortableScene2dError> {
    u64::from(y)
        .checked_mul(u64::from(width))
        .and_then(|row| row.checked_add(u64::from(x)))
        .ok_or(PortableScene2dError::ArithmeticOverflow)
}

fn clamp_z_bucket(z: i64) -> i32 {
    i32::try_from(z).unwrap_or(if z < 0 { i32::MIN } else { i32::MAX })
}

fn view_world_bounds(view: &WireView2d) -> Result<[i64; 4], PortableScene2dError> {
    let divisor = i128::from(view.zoom_milli)
        .checked_mul(2)
        .ok_or(PortableScene2dError::ArithmeticOverflow)?;
    let half_width = i128::from(view.viewport[2])
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_div(divisor))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(PortableScene2dError::ArithmeticOverflow)?;
    let half_height = i128::from(view.viewport[3])
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_div(divisor))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(PortableScene2dError::ArithmeticOverflow)?;
    Ok([
        view.camera_milli[0].saturating_sub(half_width),
        view.camera_milli[1].saturating_sub(half_height),
        half_width.saturating_mul(2),
        half_height.saturating_mul(2),
    ])
}

fn sprite_bounds(sprite: &WireSprite2d) -> [i64; 4] {
    let pivot_x =
        i128::from(sprite.size_milli[0]).saturating_mul(i128::from(sprite.anchor_milli[0])) / 1_000;
    let pivot_y =
        i128::from(sprite.size_milli[1]).saturating_mul(i128::from(sprite.anchor_milli[1])) / 1_000;
    [
        sprite.position_milli[0].saturating_sub(clamp_i128(pivot_x)),
        sprite.position_milli[1].saturating_sub(clamp_i128(pivot_y)),
        i64::from(sprite.size_milli[0]),
        i64::from(sprite.size_milli[1]),
    ]
}

fn text_bounds(text: &WireText2d) -> [i64; 4] {
    let estimated_width = i64::try_from(text.text.len())
        .unwrap_or(i64::MAX)
        .saturating_mul(i64::from(text.size_milli))
        .saturating_div(2);
    let width = if text.max_width_milli == 0 {
        estimated_width
    } else {
        estimated_width.min(i64::from(text.max_width_milli))
    }
    .max(1);
    [
        text.position_milli[0],
        text.position_milli[1],
        width,
        i64::from(text.size_milli),
    ]
}

fn tile_chunk_bounds(chunk: &PortableTileChunk2d) -> [i64; 4] {
    [
        chunk.origin_milli[0],
        chunk.origin_milli[1],
        i64::from(chunk.width).saturating_mul(i64::from(chunk.cell_size_milli[0])),
        i64::from(chunk.height).saturating_mul(i64::from(chunk.cell_size_milli[1])),
    ]
}

fn intersects(left: [i64; 4], right: [i64; 4]) -> bool {
    left[2] > 0
        && left[3] > 0
        && right[2] > 0
        && right[3] > 0
        && left[0] <= right[0].saturating_add(right[2])
        && left[0].saturating_add(left[2]) >= right[0]
        && left[1] <= right[1].saturating_add(right[3])
        && left[1].saturating_add(left[3]) >= right[1]
}

fn particle_bounds(particle: &PortableParticle2d) -> [i64; 4] {
    let half = i64::from(particle.size_milli / 2);
    [
        particle.position_milli[0].saturating_sub(half),
        particle.position_milli[1].saturating_sub(half),
        i64::from(particle.size_milli),
        i64::from(particle.size_milli),
    ]
}

fn draw_z(draw: PortableDraw2d<'_>) -> i64 {
    match draw {
        PortableDraw2d::Sprite(value) => value.z,
        PortableDraw2d::Shape(value) => value.z,
        PortableDraw2d::Text(value) => value.z,
        PortableDraw2d::TileChunk(value) => value.z,
        PortableDraw2d::Particle(value) => value.z,
    }
}

fn draw_id(draw: PortableDraw2d<'_>) -> u64 {
    match draw {
        PortableDraw2d::Sprite(value) => value.id,
        PortableDraw2d::Shape(value) => value.id,
        PortableDraw2d::Text(value) => value.id,
        PortableDraw2d::TileChunk(value) => value.id,
        PortableDraw2d::Particle(value) => value.serial,
    }
}

fn clamp_i128(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

fn textured_command(
    bounds: [i64; 4],
    view: &WireView2d,
    region: TextureRegion2d,
    tint: [u8; 4],
) -> Option<DrawCommand> {
    let start = world_to_screen([bounds[0], bounds[1]], view)?;
    let end = world_to_screen(
        [
            bounds[0].saturating_add(bounds[2]),
            bounds[1].saturating_add(bounds[3]),
        ],
        view,
    )?;
    let original_left = start[0].min(end[0]);
    let original_top = start[1].min(end[1]);
    let original_right = start[0].max(end[0]);
    let original_bottom = start[1].max(end[1]);
    let original_width = original_right.checked_sub(original_left)?;
    let original_height = original_bottom.checked_sub(original_top)?;
    if original_width == 0 || original_height == 0 {
        return None;
    }
    let left = original_left.max(0);
    let top = original_top.max(0);
    let right = original_right.min(i64::from(view.viewport[2]));
    let bottom = original_bottom.min(i64::from(view.viewport[3]));
    let width = u32::try_from(right.checked_sub(left)?).ok()?;
    let height = u32::try_from(bottom.checked_sub(top)?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let source_left = u64::from(region.source[0]).checked_add(
        u64::try_from(left.checked_sub(original_left)?)
            .ok()?
            .checked_mul(u64::from(region.source[2]))?
            / u64::try_from(original_width).ok()?,
    )?;
    let source_top = u64::from(region.source[1]).checked_add(
        u64::try_from(top.checked_sub(original_top)?)
            .ok()?
            .checked_mul(u64::from(region.source[3]))?
            / u64::try_from(original_height).ok()?,
    )?;
    let source_right = u64::from(region.source[0]).checked_add(
        u64::try_from(right.checked_sub(original_left)?)
            .ok()?
            .checked_mul(u64::from(region.source[2]))?
            / u64::try_from(original_width).ok()?,
    )?;
    let source_bottom = u64::from(region.source[1]).checked_add(
        u64::try_from(bottom.checked_sub(original_top)?)
            .ok()?
            .checked_mul(u64::from(region.source[3]))?
            / u64::try_from(original_height).ok()?,
    )?;
    Some(DrawCommand::TexturedRect {
        texture: region.texture,
        x: view.viewport[0].saturating_add(u32::try_from(left).ok()?),
        y: view.viewport[1].saturating_add(u32::try_from(top).ok()?),
        width,
        height,
        source_x: u32::try_from(source_left).ok()?,
        source_y: u32::try_from(source_top).ok()?,
        source_width: u32::try_from(source_right.checked_sub(source_left)?)
            .ok()?
            .max(1),
        source_height: u32::try_from(source_bottom.checked_sub(source_top)?)
            .ok()?
            .max(1),
        tint,
    })
}

fn solid_command(bounds: [i64; 4], view: &WireView2d, color: [u8; 4]) -> Option<DrawCommand> {
    let start = world_to_screen([bounds[0], bounds[1]], view)?;
    let end = world_to_screen(
        [
            bounds[0].saturating_add(bounds[2]),
            bounds[1].saturating_add(bounds[3]),
        ],
        view,
    )?;
    let left = start[0].min(end[0]).max(0);
    let top = start[1].min(end[1]).max(0);
    let right = start[0].max(end[0]).min(i64::from(view.viewport[2]));
    let bottom = start[1].max(end[1]).min(i64::from(view.viewport[3]));
    let width = u32::try_from(right.checked_sub(left)?).ok()?;
    let height = u32::try_from(bottom.checked_sub(top)?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(DrawCommand::SolidRect {
        x: view.viewport[0].checked_add(u32::try_from(left).ok()?)?,
        y: view.viewport[1].checked_add(u32::try_from(top).ok()?)?,
        width,
        height,
        color,
    })
}

fn tessellate_shape(
    shape: &WireShape2d,
    view: &WireView2d,
    commands: &mut Vec<DrawCommand>,
) -> Result<(), PortableScene2dError> {
    let polygon = match shape.kind {
        WireShapeKind2d::Rect => bounds_polygon(shape.bounds_milli),
        WireShapeKind2d::RoundedRect => {
            rounded_rect_polygon(shape.bounds_milli, i64::from(shape.radius_milli), 6)
        }
        WireShapeKind2d::Circle => ellipse_polygon(shape.bounds_milli, 32),
        WireShapeKind2d::Line => {
            let from = [shape.bounds_milli[0], shape.bounds_milli[1]];
            let to = [
                shape.bounds_milli[0].saturating_add(shape.bounds_milli[2]),
                shape.bounds_milli[1].saturating_add(shape.bounds_milli[3]),
            ];
            line_polygon(from, to, i64::from(shape.stroke_milli.max(1)))
        }
        WireShapeKind2d::Path => shape
            .path_milli
            .chunks_exact(2)
            .map(|point| [point[0], point[1]])
            .collect(),
    };
    if polygon.len() < 3 {
        return Err(PortableScene2dError::InvalidDescriptor);
    }
    let points = polygon
        .iter()
        .map(|point| world_to_screen(*point, view).ok_or(PortableScene2dError::ArithmeticOverflow))
        .collect::<Result<Vec<_>, _>>()?;
    if shape.kind != WireShapeKind2d::Line && shape.fill[3] != 0 {
        append_polygon_triangles(&points, shape.fill, view, commands)?;
    }
    if shape.kind != WireShapeKind2d::Line && shape.stroke_milli != 0 && shape.stroke[3] != 0 {
        for edge in polygon_edges(&polygon) {
            let outline = line_polygon(edge[0], edge[1], i64::from(shape.stroke_milli));
            let outline = outline
                .iter()
                .map(|point| {
                    world_to_screen(*point, view).ok_or(PortableScene2dError::ArithmeticOverflow)
                })
                .collect::<Result<Vec<_>, _>>()?;
            append_polygon_triangles(&outline, shape.stroke, view, commands)?;
        }
    } else if shape.kind == WireShapeKind2d::Line {
        append_polygon_triangles(&points, shape.stroke, view, commands)?;
    }
    Ok(())
}

fn append_polygon_triangles(
    points: &[[i64; 2]],
    color: [u8; 4],
    view: &WireView2d,
    commands: &mut Vec<DrawCommand>,
) -> Result<(), PortableScene2dError> {
    for triangle in triangulate_polygon(points)? {
        commands.push(DrawCommand::SolidTriangle {
            points: triangle.map(|point| {
                [
                    clamp_i64_to_i32(point[0].saturating_add(i64::from(view.viewport[0]))),
                    clamp_i64_to_i32(point[1].saturating_add(i64::from(view.viewport[1]))),
                ]
            }),
            color,
        });
    }
    Ok(())
}

fn world_to_screen(point: [i64; 2], view: &WireView2d) -> Option<[i64; 2]> {
    let x = i128::from(point[0])
        .checked_sub(i128::from(view.camera_milli[0]))?
        .checked_mul(i128::from(view.zoom_milli))?
        .checked_div(1_000_000)?
        .checked_add(i128::from(view.viewport[2] / 2))?;
    let y = i128::from(point[1])
        .checked_sub(i128::from(view.camera_milli[1]))?
        .checked_mul(i128::from(view.zoom_milli))?
        .checked_div(1_000_000)?
        .checked_add(i128::from(view.viewport[3] / 2))?;
    Some([i64::try_from(x).ok()?, i64::try_from(y).ok()?])
}

fn bounds_polygon(bounds: [i64; 4]) -> Vec<[i64; 2]> {
    let right = bounds[0].saturating_add(bounds[2]);
    let bottom = bounds[1].saturating_add(bounds[3]);
    vec![
        [bounds[0], bounds[1]],
        [right, bounds[1]],
        [right, bottom],
        [bounds[0], bottom],
    ]
}

fn rounded_rect_polygon(bounds: [i64; 4], radius: i64, segments: usize) -> Vec<[i64; 2]> {
    let radius = radius.max(0).min(bounds[2].min(bounds[3]) / 2);
    if radius == 0 {
        return bounds_polygon(bounds);
    }
    let centers = [
        [
            bounds[0].saturating_add(radius),
            bounds[1].saturating_add(radius),
        ],
        [
            bounds[0].saturating_add(bounds[2]).saturating_sub(radius),
            bounds[1].saturating_add(radius),
        ],
        [
            bounds[0].saturating_add(bounds[2]).saturating_sub(radius),
            bounds[1].saturating_add(bounds[3]).saturating_sub(radius),
        ],
        [
            bounds[0].saturating_add(radius),
            bounds[1].saturating_add(bounds[3]).saturating_sub(radius),
        ],
    ];
    let mut points = Vec::with_capacity(segments * 4);
    for (corner, center) in centers.into_iter().enumerate() {
        let start = std::f64::consts::PI * (corner as f64 / 2.0 + 1.0);
        for segment in 0..segments {
            let angle =
                start + std::f64::consts::FRAC_PI_2 * segment as f64 / (segments - 1) as f64;
            points.push([
                center[0].saturating_add((angle.cos() * radius as f64).round() as i64),
                center[1].saturating_add((angle.sin() * radius as f64).round() as i64),
            ]);
        }
    }
    points
}

fn ellipse_polygon(bounds: [i64; 4], segments: usize) -> Vec<[i64; 2]> {
    let center = [
        bounds[0].saturating_add(bounds[2] / 2),
        bounds[1].saturating_add(bounds[3] / 2),
    ];
    let radius = [bounds[2] as f64 / 2.0, bounds[3] as f64 / 2.0];
    (0..segments)
        .map(|segment| {
            let angle = std::f64::consts::TAU * segment as f64 / segments as f64;
            [
                center[0].saturating_add((angle.cos() * radius[0]).round() as i64),
                center[1].saturating_add((angle.sin() * radius[1]).round() as i64),
            ]
        })
        .collect()
}

fn line_polygon(from: [i64; 2], to: [i64; 2], width: i64) -> Vec<[i64; 2]> {
    let dx = to[0] as f64 - from[0] as f64;
    let dy = to[1] as f64 - from[1] as f64;
    let length = (dx * dx + dy * dy).sqrt().max(1.0);
    let normal = [
        (-dy / length * width as f64 / 2.0).round() as i64,
        (dx / length * width as f64 / 2.0).round() as i64,
    ];
    vec![
        [
            from[0].saturating_add(normal[0]),
            from[1].saturating_add(normal[1]),
        ],
        [
            to[0].saturating_add(normal[0]),
            to[1].saturating_add(normal[1]),
        ],
        [
            to[0].saturating_sub(normal[0]),
            to[1].saturating_sub(normal[1]),
        ],
        [
            from[0].saturating_sub(normal[0]),
            from[1].saturating_sub(normal[1]),
        ],
    ]
}

fn triangulate_polygon(points: &[[i64; 2]]) -> Result<Vec<[[i64; 2]; 3]>, PortableScene2dError> {
    if points.len() < 3 {
        return Err(PortableScene2dError::InvalidDescriptor);
    }
    let mut points = points.to_vec();
    if points.first() == points.last() {
        points.pop();
    }
    points.dedup();
    if points.len() < 3 {
        return Err(PortableScene2dError::InvalidDescriptor);
    }
    let area = signed_polygon_area(&points);
    if area == 0 {
        return Err(PortableScene2dError::InvalidDescriptor);
    }
    let counter_clockwise = area > 0;
    let mut remaining = (0..points.len()).collect::<Vec<_>>();
    let mut triangles = Vec::with_capacity(points.len() - 2);
    while remaining.len() > 3 {
        let mut ear = None;
        for cursor in 0..remaining.len() {
            let previous = remaining[(cursor + remaining.len() - 1) % remaining.len()];
            let current = remaining[cursor];
            let next = remaining[(cursor + 1) % remaining.len()];
            let turn = cross(points[previous], points[current], points[next]);
            if turn == 0 || (turn > 0) != counter_clockwise {
                continue;
            }
            if remaining.iter().copied().any(|candidate| {
                candidate != previous
                    && candidate != current
                    && candidate != next
                    && point_in_triangle(
                        points[candidate],
                        points[previous],
                        points[current],
                        points[next],
                    )
            }) {
                continue;
            }
            ear = Some((cursor, [points[previous], points[current], points[next]]));
            break;
        }
        let (cursor, triangle) = ear.ok_or(PortableScene2dError::InvalidDescriptor)?;
        triangles.push(triangle);
        remaining.remove(cursor);
    }
    triangles.push([
        points[remaining[0]],
        points[remaining[1]],
        points[remaining[2]],
    ]);
    Ok(triangles)
}

fn polygon_edges(points: &[[i64; 2]]) -> impl Iterator<Item = [[i64; 2]; 2]> + '_ {
    points
        .iter()
        .copied()
        .zip(points.iter().copied().cycle().skip(1))
        .take(points.len())
        .map(|(from, to)| [from, to])
}

fn signed_polygon_area(points: &[[i64; 2]]) -> i128 {
    polygon_edges(points).fold(0_i128, |area, [from, to]| {
        area + i128::from(from[0]) * i128::from(to[1]) - i128::from(to[0]) * i128::from(from[1])
    })
}

fn cross(from: [i64; 2], pivot: [i64; 2], to: [i64; 2]) -> i128 {
    (i128::from(pivot[0]) - i128::from(from[0])) * (i128::from(to[1]) - i128::from(pivot[1]))
        - (i128::from(pivot[1]) - i128::from(from[1])) * (i128::from(to[0]) - i128::from(pivot[0]))
}

fn point_in_triangle(point: [i64; 2], first: [i64; 2], second: [i64; 2], third: [i64; 2]) -> bool {
    let signs = [
        cross(first, second, point),
        cross(second, third, point),
        cross(third, first, point),
    ];
    signs.iter().all(|sign| *sign >= 0) || signs.iter().all(|sign| *sign <= 0)
}

fn clamp_i64_to_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

fn split_mix64_2d(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn random_i64_2d(random: u64, minimum: i64, maximum: i64) -> Result<i64, PortableScene2dError> {
    if minimum > maximum {
        return Err(PortableScene2dError::InvalidDescriptor);
    }
    let span = (i128::from(maximum) - i128::from(minimum) + 1) as u128;
    let offset = u128::from(random) % span;
    i64::try_from(i128::from(minimum) + offset as i128)
        .map_err(|_| PortableScene2dError::ArithmeticOverflow)
}

fn interpolate_color(from: [u8; 4], to: [u8; 4], elapsed: u32, duration: u32) -> [u8; 4] {
    let mut color = [0; 4];
    for channel in 0..4 {
        color[channel] = interpolate_size(
            u32::from(from[channel]),
            u32::from(to[channel]),
            elapsed,
            duration,
        ) as u8;
    }
    color
}

fn interpolate_size(from: u32, to: u32, elapsed: u32, duration: u32) -> u32 {
    if duration == 0 {
        return to;
    }
    let from = i128::from(from);
    let value =
        from + (i128::from(to) - from) * i128::from(elapsed.min(duration)) / i128::from(duration);
    u32::try_from(value).unwrap_or(if value < 0 { 0 } else { u32::MAX })
}

#[allow(dead_code)]
fn _assert_update_is_public(_: WireTileChunk2d) {}
