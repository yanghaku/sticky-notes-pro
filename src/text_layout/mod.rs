//! Shaping, drawing and hit testing for the structured editor.
//!
//! Public coordinates are logical pixels; immutable images are rasterized at the
//! window's physical scale. List markers occupy a separate, noneditable gutter.

mod geometry;
mod raster;
mod resources;

use crate::editor::{EditorRow, Position};
use cosmic_text::{Attrs, Buffer, Cursor, Family, Metrics, Shaping, Wrap};
use geometry::{RowLayout, boundary, buffer_matches, finite, key};
use raster::{Canvas, EditorPalette, MAX_PIXEL_SIDE, MAX_PIXELS};
pub use resources::TextResources;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::{borrow::Cow, cell::RefCell, rc::Rc};

const PADDING: f32 = 14.0;
const GUTTER: f32 = 38.0;

pub struct Rendered {
    pub image: Image,
    /// Caret top-left, in logical coordinates relative to the editor image.
    pub caret: (f32, f32),
    /// Effective logical scroll, clamped and optionally adjusted to reveal the caret.
    pub scroll: f32,
}

/// Geometry prepared without allocating or drawing an editor image.
/// The shaped rows remain in `TextLayout` for navigation and hit testing.
pub struct PreparedLayout {
    /// Caret top-left, in logical coordinates relative to the editor viewport.
    pub caret: (f32, f32),
    /// Effective logical scroll, clamped and optionally adjusted to reveal the caret.
    pub scroll: f32,
    pixel_width: u32,
    pixel_height: u32,
    caret_pixels: (f32, f32),
    metrics: Metrics,
}

pub struct TextLayout {
    resources: Rc<RefCell<TextResources>>,
    rows: Vec<RowLayout>,
    scale: f32,
    scroll: f32,
    line_height: f32,
    preedit_cursor: Option<(usize, usize)>,
    font_family: String,
    font_family_dirty: bool,
    dark: bool,
}

impl TextLayout {
    pub fn new(resources: Rc<RefCell<TextResources>>) -> Self {
        Self {
            resources,
            rows: Vec::new(),
            scale: 1.0,
            scroll: 0.0,
            line_height: 27.0,
            preedit_cursor: None,
            font_family: String::new(),
            font_family_dirty: true,
            dark: false,
        }
    }

    /// Change drawing colors without invalidating shaping or shared glyph caches.
    /// The caller schedules a render after changing the application appearance.
    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
    }

    /// Select the global editor family. Empty uses the system sans-serif family;
    /// missing named fonts retain cosmic-text's normal fallback behavior.
    pub fn set_font_family(&mut self, family: &str) {
        let family = family.trim();
        if self.font_family != family {
            self.font_family.clear();
            self.font_family.push_str(family);
            self.font_family_dirty = true;
        }
    }

    /// Byte range supplied by winit's IME Preedit event. None puts the caret at
    /// the composition end. This modifies only temporary presentation state.
    pub fn set_preedit_cursor(&mut self, cursor: Option<(usize, usize)>) -> bool {
        let changed = self.preedit_cursor != cursor;
        self.preedit_cursor = cursor;
        changed
    }

    /// Refresh shaping, navigation geometry and scroll without rasterizing.
    /// Raster limits still determine the geometry's effective physical scale,
    /// so preparing and rendering use identical coordinates at extreme sizes.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        rows: &[EditorRow<'_>],
        selection: (Position, Position),
        preedit: &str,
        width: f32,
        height: f32,
        font_size: f32,
        scale: f32,
        scroll: f32,
        follow_caret: bool,
    ) -> PreparedLayout {
        let width = finite(width, 1.0).clamp(1.0, 32768.0);
        let height = finite(height, 1.0).clamp(1.0, 32768.0);
        let requested_scale = finite(scale, 1.0).clamp(0.5, 8.0);
        let budget_scale = (MAX_PIXELS as f32 / (width * height)).sqrt();
        self.scale = requested_scale
            .min(budget_scale)
            .min(MAX_PIXEL_SIDE / width)
            .min(MAX_PIXEL_SIDE / height);
        self.scroll = finite(scroll, 0.0).max(0.0) * self.scale;
        let font_size = finite(font_size, 18.0).clamp(8.0, 96.0);
        self.line_height = font_size * 1.5 * self.scale;
        let metrics = Metrics::new(font_size * self.scale, self.line_height);
        let pixel_width = (width * self.scale).floor().max(1.0) as u32;
        let pixel_height =
            ((height * self.scale).floor().max(1.0) as u32).min(MAX_PIXELS / pixel_width);
        self.rows.truncate(rows.len());
        if self.rows.capacity() > self.rows.len().saturating_mul(4) && self.rows.capacity() > 1024 {
            self.rows.shrink_to_fit();
        }
        let mut resources = self.resources.borrow_mut();
        let family = if self.font_family.is_empty() {
            Family::SansSerif
        } else {
            Family::Name(&self.font_family)
        };
        let attrs = Attrs::new().family(family);
        let mut top = PADDING * self.scale;
        let mut caret = (top, top);

        for (index, row) in rows.iter().enumerate() {
            let new_row = index == self.rows.len();
            if new_row {
                let mut body = Buffer::new_empty(metrics);
                body.set_wrap(Wrap::WordOrGlyph);
                let mut marker = Buffer::new_empty(metrics);
                marker.set_wrap(Wrap::None);
                self.rows.push(RowLayout {
                    body,
                    marker,
                    metrics,
                    width: 0.0,
                    visual_lines: Vec::new(),
                    top,
                    height: self.line_height,
                    left: 0.0,
                    source_len: 0,
                    preedit: None,
                });
            }
            let layout = &mut self.rows[index];
            let left = (PADDING + if row.marker.is_empty() { 0.0 } else { GUTTER }) * self.scale;
            let available =
                (pixel_width as f32 - left - PADDING * self.scale).max(metrics.font_size);
            let mut text = Cow::Borrowed(row.text);
            layout.preedit = None;
            if index == selection.1.row && !preedit.is_empty() {
                let offset = boundary(&text, selection.1.offset);
                text.to_mut().insert_str(offset, preedit);
                layout.preedit = Some((offset, preedit.len()));
            }
            // Release a formerly huge paragraph's retained text and glyph storage
            // after a substantial deletion, rather than keeping its peak capacity.
            let reset_body = layout.body.lines.first().is_some_and(|line| {
                line.text().len() > 256 * 1024 && line.text().len() > text.len().saturating_mul(4)
            });
            if reset_body {
                layout.body = Buffer::new_empty(metrics);
                layout.body.set_wrap(Wrap::WordOrGlyph);
            }
            let metrics_changed = new_row || layout.metrics != metrics;
            let body_settings_changed = metrics_changed || reset_body || layout.width != available;
            if body_settings_changed {
                layout
                    .body
                    .set_metrics_and_size(metrics, Some(available), None);
            }
            if metrics_changed {
                layout
                    .marker
                    .set_metrics_and_size(metrics, Some(GUTTER * self.scale), None);
            }
            let body_text_changed = new_row
                || reset_body
                || self.font_family_dirty
                || !buffer_matches(&layout.body, &text);
            if body_text_changed {
                layout.body.set_text(&text, &attrs, Shaping::Advanced, None);
            }
            let marker_text_changed =
                new_row || self.font_family_dirty || !buffer_matches(&layout.marker, row.marker);
            if marker_text_changed {
                layout
                    .marker
                    .set_text(row.marker, &attrs, Shaping::Advanced, None);
            }
            if body_settings_changed || body_text_changed {
                layout.body.shape_until_scroll(&mut resources.fonts, false);
                layout.rebuild_visual_lines(self.line_height);
            }
            if metrics_changed || marker_text_changed {
                layout
                    .marker
                    .shape_until_scroll(&mut resources.fonts, false);
            }
            layout.metrics = metrics;
            layout.width = available;
            layout.top = top;
            layout.left = left;
            layout.source_len = row.text.len();
            if index == selection.1.row {
                let offset = layout.preedit.map_or(selection.1.offset, |(at, length)| {
                    at + self
                        .preedit_cursor
                        .map_or(length, |(_, end)| boundary(preedit, end))
                });
                if let Some((x, y, _)) = layout.cursor_point(offset) {
                    caret = (left + x, top + y);
                }
            }
            top += layout.height + 3.0 * self.scale;
        }
        self.font_family_dirty = false;
        let content_height = top + PADDING * self.scale;
        let viewport_height = height * self.scale;
        let max_scroll = (content_height - viewport_height).max(0.0);
        self.scroll = self.scroll.min(max_scroll);
        if follow_caret {
            if caret.1 < self.scroll {
                self.scroll = caret.1;
            } else if caret.1 + self.line_height > self.scroll + viewport_height {
                self.scroll = caret.1 + self.line_height - viewport_height;
            }
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        caret.1 -= self.scroll;
        PreparedLayout {
            caret: (caret.0 / self.scale, caret.1 / self.scale),
            scroll: self.scroll / self.scale,
            pixel_width,
            pixel_height,
            caret_pixels: caret,
            metrics,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        rows: &[EditorRow<'_>],
        selection: (Position, Position),
        preedit: &str,
        width: f32,
        height: f32,
        font_size: f32,
        scale: f32,
        scroll: f32,
        follow_caret: bool,
    ) -> Rendered {
        // Final geometry is settled first. Navigation can use prepare alone;
        // rendering allocates and paints exactly one raster after preparation.
        let prepared = self.prepare(
            rows,
            selection,
            preedit,
            width,
            height,
            font_size,
            scale,
            scroll,
            follow_caret,
        );
        let palette = EditorPalette::for_dark(self.dark);
        let (start, end) = if key(selection.0) <= key(selection.1) {
            (selection.0, selection.1)
        } else {
            (selection.1, selection.0)
        };
        let pixel_width = prepared.pixel_width;
        let pixel_height = prepared.pixel_height;
        let caret = prepared.caret_pixels;
        let metrics = prepared.metrics;
        let mut resources = self.resources.borrow_mut();
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(pixel_width, pixel_height);
        let mut canvas = Canvas {
            bytes: pixels.make_mut_bytes(),
            width: pixel_width,
            height: pixel_height,
        };
        let first_visible = self
            .rows
            .partition_point(|row| row.top + row.height <= self.scroll);
        for (index, layout) in self.rows.iter().enumerate().skip(first_visible) {
            let row = &rows[index];
            let left = layout.left;
            let draw_y = layout.top - self.scroll;
            if draw_y >= pixel_height as f32 {
                break;
            }
            let visible = draw_y < pixel_height as f32 && draw_y + layout.height > 0.0;

            if visible && start != end && index >= start.row && index <= end.row {
                let from = if index == start.row { start.offset } else { 0 };
                let to = if index == end.row {
                    end.offset
                } else {
                    row.text.len()
                };
                for (run_index, run) in layout.body.layout_runs().enumerate() {
                    if draw_y + run.line_top + run.line_height <= 0.0 {
                        continue;
                    }
                    if draw_y + run.line_top >= pixel_height as f32 {
                        break;
                    }
                    for (x, w) in run.highlight(
                        Cursor::new(0, layout.display_offset(from)),
                        Cursor::new(0, layout.display_offset(to)),
                    ) {
                        canvas.rect(
                            left + x,
                            draw_y + run.line_top,
                            w,
                            run.line_height,
                            palette.selection,
                        );
                    }
                    // Highlight the paragraph break as well, including empty items.
                    if index < end.row && run_index + 1 == layout.visual_lines.len() {
                        canvas.rect(
                            left + run.line_w,
                            draw_y + run.line_top,
                            metrics.font_size * 0.45,
                            run.line_height,
                            palette.selection,
                        );
                    }
                }
            }
            if visible && let Some((at, length)) = layout.preedit {
                for run in layout.body.layout_runs() {
                    if draw_y + run.line_top + run.line_height <= 0.0 {
                        continue;
                    }
                    if draw_y + run.line_top >= pixel_height as f32 {
                        break;
                    }
                    if let Some((start, end)) = self.preedit_cursor {
                        let start = boundary(preedit, start);
                        let end = boundary(preedit, end);
                        for (x, w) in run.highlight(
                            Cursor::new(0, at + start.min(end)),
                            Cursor::new(0, at + start.max(end)),
                        ) {
                            canvas.rect(
                                left + x,
                                draw_y + run.line_top,
                                w,
                                run.line_height,
                                palette.selection,
                            );
                        }
                    }
                    for (x, w) in run.highlight(Cursor::new(0, at), Cursor::new(0, at + length)) {
                        canvas.rect(
                            left + x,
                            draw_y + run.line_top + run.line_height - 2.0 * self.scale,
                            w,
                            self.scale,
                            palette.ime_underline,
                        );
                    }
                }
            }
            if visible {
                resources.draw_visible(&layout.body, &mut canvas, left, draw_y, palette.body);
                resources.draw_visible(
                    &layout.marker,
                    &mut canvas,
                    PADDING * self.scale,
                    draw_y,
                    palette.marker,
                );
            }
        }
        canvas.rect(
            caret.0,
            caret.1,
            self.scale.max(1.0),
            self.line_height,
            palette.body.as_rgba(),
        );
        Rendered {
            image: Image::from_rgba8(pixels),
            caret: prepared.caret,
            scroll: prepared.scroll,
        }
    }
}
