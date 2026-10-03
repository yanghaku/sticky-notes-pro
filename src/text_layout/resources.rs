//! Shared font discovery, bounded bitmap cache, and visible glyph drawing.

use super::raster::Canvas;
use cosmic_text::{
    Buffer, CacheKey, Color, FontSystem, PhysicalGlyph, Renderer, SwashCache, SwashImage,
};
use std::{
    collections::{BTreeSet, VecDeque},
    mem::size_of,
};

const MAX_GLYPH_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_GLYPH_CACHE_ENTRIES: usize = 16_384;
const CACHE_EVICTIONS_PER_MISS: usize = 8;

/// Font discovery and rasterized glyphs shared by all Note windows on the UI thread.
pub struct TextResources {
    pub(super) fonts: FontSystem,
    cache: SwashCache,
    glyph_order: VecDeque<(CacheKey, usize)>,
    glyph_bytes: usize,
}

impl Default for TextResources {
    fn default() -> Self {
        Self::new()
    }
}

impl TextResources {
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
            glyph_order: VecDeque::new(),
            glyph_bytes: 0,
        }
    }

    /// Enumerate installed families without repeating system font discovery.
    pub fn available_font_families(&self) -> Vec<String> {
        self.fonts
            .db()
            .faces()
            .flat_map(|face| face.families.iter().map(|(family, _)| family.as_str()))
            .filter(|family| !family.trim().is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn cache_has_room(&self, image_bytes: usize) -> bool {
        // O(1) incremental accounting, including retained table and queue storage.
        // This bounds the bitmap cache, not fonts, layouts or process RSS.
        let table_bytes = self
            .cache
            .image_cache
            .capacity()
            .saturating_mul(size_of::<(CacheKey, Option<SwashImage>)>() + 1);
        let queue_capacity = self
            .glyph_order
            .capacity()
            .max(self.glyph_order.len().saturating_add(1))
            .next_power_of_two();
        let queue_bytes = queue_capacity.saturating_mul(size_of::<(CacheKey, usize)>());
        self.cache.image_cache.len() <= MAX_GLYPH_CACHE_ENTRIES
            && self
                .glyph_bytes
                .saturating_add(image_bytes)
                .saturating_add(table_bytes)
                .saturating_add(queue_bytes)
                <= MAX_GLYPH_CACHE_BYTES
    }

    /// Return whether the new bitmap is transient and must be discarded after drawing.
    fn prepare_glyph(&mut self, key: CacheKey) -> bool {
        if self.cache.image_cache.contains_key(&key) {
            return false;
        }
        let image_bytes = self
            .cache
            .get_image(&mut self.fonts, key)
            .as_ref()
            .map_or(0, |image| image.data.capacity());
        if image_bytes > MAX_GLYPH_CACHE_BYTES / 4 {
            return true;
        }
        // A large newcomer cannot cause a full-cache drop in the input path.
        // Evict a small FIFO batch; if it still cannot fit, draw without retaining it.
        for _ in 0..CACHE_EVICTIONS_PER_MISS {
            if self.cache_has_room(image_bytes) {
                break;
            }
            let Some((old_key, old_bytes)) = self.glyph_order.pop_front() else {
                break;
            };
            self.cache.image_cache.remove(&old_key);
            self.glyph_bytes = self.glyph_bytes.saturating_sub(old_bytes);
        }
        if !self.cache_has_room(image_bytes) {
            return true;
        }
        self.glyph_order.push_back((key, image_bytes));
        self.glyph_bytes = self.glyph_bytes.saturating_add(image_bytes);
        false
    }

    fn draw_glyph(
        &mut self,
        canvas: &mut Canvas<'_>,
        glyph: PhysicalGlyph,
        color: Color,
        origin: (i32, i32),
    ) {
        let transient = self.prepare_glyph(glyph.cache_key);
        let x = origin.0 + glyph.x;
        let y = origin.1 + glyph.y;
        self.cache
            .with_pixels(&mut self.fonts, glyph.cache_key, color, |dx, dy, pixel| {
                canvas.pixel(x + dx, y + dy, pixel.as_rgba());
            });
        if transient {
            self.cache.image_cache.remove(&glyph.cache_key);
        }
    }

    pub(super) fn draw_visible(
        &mut self,
        buffer: &Buffer,
        canvas: &mut Canvas<'_>,
        left: f32,
        top: f32,
        color: Color,
    ) {
        for run in buffer.layout_runs() {
            if top + run.line_top + run.line_height <= 0.0 {
                continue;
            }
            if top + run.line_top >= canvas.height as f32 {
                break;
            }
            {
                let mut renderer = GlyphRenderer {
                    resources: self,
                    canvas,
                    origin: (left.round() as i32, top.round() as i32),
                };
                for glyph in run.glyphs {
                    renderer.glyph(
                        glyph.physical((0.0, run.line_y), 1.0),
                        glyph.color_opt.unwrap_or(color),
                    );
                }
                cosmic_text::render_decoration(&mut renderer, &run, color);
            }
        }
    }
}

struct GlyphRenderer<'a, 'pixels> {
    resources: &'a mut TextResources,
    canvas: &'a mut Canvas<'pixels>,
    origin: (i32, i32),
}

impl Renderer for GlyphRenderer<'_, '_> {
    fn rectangle(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        self.canvas.rect(
            (self.origin.0 + x) as f32,
            (self.origin.1 + y) as f32,
            width as f32,
            height as f32,
            color.as_rgba(),
        );
    }
    fn glyph(&mut self, glyph: PhysicalGlyph, color: Color) {
        self.resources
            .draw_glyph(self.canvas, glyph, color, self.origin);
    }
}
