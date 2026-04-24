//! Dynamic font manager using fontdue.
//!
//! Replaces the old 8x8 bitmap font with proper TTF/OTF rendering.
//! Supports arbitrary Unicode including CJK via on-demand glyph rasterization
//! into a shelf-packed texture atlas.

use crate::file_io;
use crate::pipeline_sprite::{SpriteDraw, SpriteInstance};
use crate::texture::{TextureId, TextureManager};
use std::collections::HashMap;

/// Atlas dimensions. 2048x2048 RGBA = 16 MB, fits ~4000 glyphs at 32px.
const ATLAS_SIZE: u32 = 2048;

/// Default embedded font (Liberation Mono — compact, readable, open-source).
/// Included at compile time so text rendering works without loading any file.
const DEFAULT_FONT_DATA: &[u8] = include_bytes!("fonts/default.ttf");
const DEFAULT_FONT_PATHS: [&str; 6] = [
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Courier New.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    "C:/Windows/Fonts/arial.ttf",
];

fn load_default_font() -> Result<fontdue::Font, String> {
    let embedded_error =
        match fontdue::Font::from_bytes(DEFAULT_FONT_DATA, fontdue::FontSettings::default()) {
            Ok(font) => return Ok(font),
            Err(error) => error.to_string(),
        };

    #[cfg(not(feature = "wasm"))]
    for path in DEFAULT_FONT_PATHS {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        if let Ok(font) = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()) {
            return Ok(font);
        }
    }

    #[cfg(feature = "wasm")]
    {
        Err(format!(
            "voplay: failed to load embedded default font: {}",
            embedded_error
        ))
    }

    #[cfg(not(feature = "wasm"))]
    {
        Err(format!(
            "voplay: failed to load default font from embedded asset or system font paths; embedded font error: {}",
            embedded_error
        ))
    }
}

pub type FontId = u32;

/// Key for glyph cache lookup.
#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    font_id: FontId,
    ch: char,
    size_px: u16, // quantized pixel size
}

/// Cached glyph info — UV coords in atlas + layout metrics.
#[derive(Clone)]
struct CachedGlyph {
    // UV in atlas (normalized 0..1)
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    // Pixel metrics from fontdue
    width: f32,
    height: f32,
    x_offset: f32,
    y_offset: f32,
    advance: f32,
}

/// Shelf in the atlas packer.
struct Shelf {
    y: u32,
    height: u32,
    x_cursor: u32,
}

/// Manages loaded fonts, glyph rasterization, and the dynamic texture atlas.
pub struct FontManager {
    fonts: HashMap<FontId, fontdue::Font>,
    next_font_id: FontId,
    current_font: FontId, // 0 = default

    // Glyph cache
    cache: HashMap<GlyphKey, CachedGlyph>,

    // Atlas
    atlas_data: Vec<u8>, // RGBA, ATLAS_SIZE x ATLAS_SIZE
    atlas_texture_id: Option<TextureId>,
    atlas_dirty: bool,
    shelves: Vec<Shelf>,
}

impl FontManager {
    /// Create a new font manager and register the default embedded font.
    pub fn new() -> Result<Self, String> {
        let default_font = load_default_font()?;

        let mut fonts = HashMap::new();
        fonts.insert(0, default_font);

        Ok(Self {
            fonts,
            next_font_id: 1,
            current_font: 0,
            cache: HashMap::new(),
            atlas_data: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            atlas_texture_id: None,
            atlas_dirty: true,
            shelves: Vec::new(),
        })
    }

    /// Load a font from raw TTF/OTF bytes. Returns font ID.
    pub fn load_bytes(&mut self, data: Vec<u8>) -> Result<FontId, String> {
        let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
            .map_err(|e| format!("font parse error: {}", e))?;

        let id = self.next_font_id;
        self.next_font_id += 1;
        self.fonts.insert(id, font);
        Ok(id)
    }

    /// Load a font from a file path. Returns font ID.
    pub fn load_file(&mut self, path: &str) -> Result<FontId, String> {
        let data = file_io::read_bytes(path).map_err(|e| format!("read font '{}': {}", path, e))?;
        self.load_bytes(data)
    }

    /// Free a loaded font. Does not invalidate cached glyphs (they remain usable).
    pub fn free(&mut self, id: FontId) {
        if id == 0 {
            return; // cannot free default font
        }
        self.fonts.remove(&id);
    }

    /// Set the current font for subsequent layout_text calls.
    pub fn set_current(&mut self, id: FontId) {
        self.current_font = id;
    }

    /// Reset current font to default.
    pub fn reset_current(&mut self) {
        self.current_font = 0;
    }

    /// Ensure the atlas texture exists and is up-to-date in the TextureManager.
    pub fn ensure_atlas(
        &mut self,
        texture_manager: &mut TextureManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        match self.atlas_texture_id {
            None => {
                // First time: create atlas texture
                let id = texture_manager.load_rgba(
                    device,
                    queue,
                    ATLAS_SIZE,
                    ATLAS_SIZE,
                    &self.atlas_data,
                );
                self.atlas_texture_id = Some(id);
                self.atlas_dirty = false;
            }
            Some(id) => {
                if self.atlas_dirty {
                    texture_manager.update_rgba(queue, id, &self.atlas_data);
                    self.atlas_dirty = false;
                }
            }
        }
    }

    /// Layout a text string and return sprite draws.
    /// Rasterizes any uncached glyphs into the atlas on-demand.
    pub fn layout_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    ) -> Vec<SpriteDraw> {
        let font_id = self.current_font;
        let size_px = (size.max(1.0)) as u16;
        let atlas_tex_id = match self.atlas_texture_id {
            Some(id) => id,
            None => return Vec::new(), // atlas not yet created
        };

        let mut draws = Vec::with_capacity(text.len());
        let mut cx = x;

        for ch in text.chars() {
            let key = GlyphKey {
                font_id,
                ch,
                size_px,
            };

            // Ensure glyph is cached
            if !self.cache.contains_key(&key) {
                self.rasterize_glyph(&key);
            }

            if let Some(glyph) = self.cache.get(&key) {
                if glyph.width > 0.0 && glyph.height > 0.0 {
                    draws.push(SpriteDraw {
                        texture_id: atlas_tex_id,
                        instance: SpriteInstance {
                            dst_rect: [
                                cx + glyph.x_offset,
                                y + glyph.y_offset,
                                glyph.width,
                                glyph.height,
                            ],
                            src_rect: [glyph.u0, glyph.v0, glyph.u1, glyph.v1],
                            color: [r, g, b, a],
                            params: [0.0, 0.0, 0.0, 0.0],
                        },
                    });
                }
                cx += glyph.advance;
            } else {
                // Glyph not available (atlas full or font missing), skip
                cx += size * 0.5;
            }
        }

        draws
    }

    /// Rasterize a single glyph and pack it into the atlas.
    fn rasterize_glyph(&mut self, key: &GlyphKey) {
        let font = match self.fonts.get(&key.font_id) {
            Some(f) => f,
            None => {
                // Fallback to default font if requested font not found
                match self.fonts.get(&0) {
                    Some(f) => f,
                    None => return,
                }
            }
        };

        let size = key.size_px as f32;
        let (metrics, bitmap) = font.rasterize(key.ch, size);

        if metrics.width == 0 || metrics.height == 0 {
            // Whitespace or zero-size glyph — cache with zero dimensions
            self.cache.insert(
                key.clone(),
                CachedGlyph {
                    u0: 0.0,
                    v0: 0.0,
                    u1: 0.0,
                    v1: 0.0,
                    width: 0.0,
                    height: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                    advance: metrics.advance_width,
                },
            );
            return;
        }

        let gw = metrics.width as u32;
        let gh = metrics.height as u32;

        // Pack into atlas using shelf algorithm
        let (px, py) = match self.pack_rect(gw + 1, gh + 1) {
            Some(pos) => pos,
            None => {
                log::warn!("voplay: font atlas full, cannot cache glyph '{}'", key.ch);
                return;
            }
        };

        // Write glyph bitmap into atlas RGBA data (white with alpha from coverage)
        for gy in 0..gh {
            for gx in 0..gw {
                let alpha = bitmap[(gy * gw + gx) as usize];
                let ax = px + gx;
                let ay = py + gy;
                let idx = ((ay * ATLAS_SIZE + ax) * 4) as usize;
                self.atlas_data[idx] = 255; // R
                self.atlas_data[idx + 1] = 255; // G
                self.atlas_data[idx + 2] = 255; // B
                self.atlas_data[idx + 3] = alpha;
            }
        }
        self.atlas_dirty = true;

        let inv_w = 1.0 / ATLAS_SIZE as f32;
        let inv_h = 1.0 / ATLAS_SIZE as f32;

        self.cache.insert(
            key.clone(),
            CachedGlyph {
                u0: px as f32 * inv_w,
                v0: py as f32 * inv_h,
                u1: (px + gw) as f32 * inv_w,
                v1: (py + gh) as f32 * inv_h,
                width: gw as f32,
                height: gh as f32,
                x_offset: metrics.xmin as f32,
                y_offset: -(metrics.height as f32 + metrics.ymin as f32),
                advance: metrics.advance_width,
            },
        );
    }

    /// Measure text dimensions (width, height) using fontdue metrics.
    /// Uses the specified font_id (0 = default).
    pub fn measure_text(&self, font_id: FontId, text: &str, size: f32) -> (f32, f32) {
        let font = match self.fonts.get(&font_id) {
            Some(f) => f,
            None => match self.fonts.get(&0) {
                Some(f) => f,
                None => return (0.0, 0.0),
            },
        };

        let mut width: f32 = 0.0;
        let mut max_height: f32 = 0.0;
        for ch in text.chars() {
            let metrics = font.metrics(ch, size);
            width += metrics.advance_width;
            let h = metrics.height as f32;
            if h > max_height {
                max_height = h;
            }
        }

        // Use line metrics if available for more accurate height
        let line_height = font
            .horizontal_line_metrics(size)
            .map(|lm| lm.ascent - lm.descent)
            .unwrap_or(max_height);

        (width, line_height)
    }

    /// Shelf-based rectangle packing. Returns top-left (x, y) or None if full.
    fn pack_rect(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Try to fit in an existing shelf
        for shelf in &mut self.shelves {
            if h <= shelf.height && shelf.x_cursor + w <= ATLAS_SIZE {
                let pos = (shelf.x_cursor, shelf.y);
                shelf.x_cursor += w;
                return Some(pos);
            }
        }

        // Start a new shelf
        let new_y = if let Some(last) = self.shelves.last() {
            last.y + last.height
        } else {
            0
        };

        if new_y + h > ATLAS_SIZE {
            return None; // atlas is full
        }

        self.shelves.push(Shelf {
            y: new_y,
            height: h,
            x_cursor: w,
        });

        Some((0, new_y))
    }
}
