//! Cached row geometry, cursor mapping, and hit testing.

use super::TextLayout;
use crate::editor::Position;
use crate::model::BoldRange;
use cosmic_text::{Buffer, Cursor, Metrics};

pub(super) struct RowLayout {
    pub(super) body: Buffer,
    pub(super) marker: Buffer,
    pub(super) metrics: Metrics,
    pub(super) width: f32,
    pub(super) visual_lines: Vec<VisualLine>,
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) left: f32,
    pub(super) source_len: usize,
    pub(super) preedit: Option<(usize, usize)>,
    /// Display ranges, including any temporary IME insertion. Style-only edits
    /// must invalidate shaping even if the cached body text remains identical.
    pub(super) bold: Vec<BoldRange>,
}

pub(super) struct VisualLine {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) top: f32,
    pub(super) height: f32,
}

impl TextLayout {
    /// Hit test logical editor coordinates. Marker clicks resolve to body offset zero.
    pub fn hit(&self, x: f32, y: f32) -> Option<Position> {
        if !x.is_finite() || !y.is_finite() || self.rows.is_empty() {
            return None;
        }
        let y = y * self.scale + self.scroll;
        let index = self
            .rows
            .partition_point(|row| y >= row.top + row.height + 3.0 * self.scale)
            .min(self.rows.len() - 1);
        let row = &self.rows[index];
        if x * self.scale < row.left {
            return Some(Position {
                row: index,
                offset: 0,
            });
        }
        let cursor = row
            .body
            .hit((x * self.scale - row.left).max(0.0), (y - row.top).max(0.0))?;
        let mut offset = cursor.index;
        if let Some((at, length)) = row.preedit {
            offset = if offset <= at {
                offset
            } else if offset <= at + length {
                at
            } else {
                offset - length
            };
        }
        Some(Position {
            row: index,
            offset: offset.min(row.source_len),
        })
    }

    /// Move by one visual line while retaining the current horizontal position.
    pub fn vertical(&self, position: Position, delta: i32) -> Option<Position> {
        let row = self.rows.get(position.row)?;
        let (x, top, height) = row.cursor_point(row.display_offset(position.offset))?;
        let y = row.top + top + height * (delta as f32 + 0.5) - self.scroll;
        self.hit((row.left + x) / self.scale, y / self.scale)
    }
}

impl RowLayout {
    pub(super) fn rebuild_visual_lines(&mut self, minimum_height: f32) {
        self.visual_lines.clear();
        self.height = minimum_height;
        for run in self.body.layout_runs() {
            let start = run
                .glyphs
                .iter()
                .map(|glyph| glyph.start)
                .min()
                .unwrap_or(0);
            let end = run.glyphs.iter().map(|glyph| glyph.end).max().unwrap_or(0);
            self.visual_lines.push(VisualLine {
                start,
                end,
                top: run.line_top,
                height: run.line_height,
            });
            self.height = self.height.max(run.line_top + run.line_height);
        }
        if self.visual_lines.capacity() > self.visual_lines.len().saturating_mul(4)
            && self.visual_lines.capacity() > 1024
        {
            self.visual_lines.shrink_to_fit();
        }
    }

    pub(super) fn cursor_point(&self, offset: usize) -> Option<(f32, f32, f32)> {
        // Logical ranges are cached during reflow. A normal caret refresh does
        // not search glyphs in thousands of offscreen wraps. Prefer the later
        // visual line at a shared wrapped boundary.
        for (index, line) in self.visual_lines.iter().enumerate().rev() {
            if offset < line.start || offset > line.end {
                continue;
            }
            let run = self.body.layout_runs().nth(index)?;
            if let Some(x) = run.cursor_position(&Cursor::new(0, offset)) {
                return Some((x, line.top, line.height));
            }
            // Mixed bidi ranges can overlap without containing this particular
            // cluster; try the previous qualifying range in that rare case.
        }
        None
    }

    pub(super) fn display_offset(&self, offset: usize) -> usize {
        self.preedit.map_or(
            offset,
            |(at, length)| if offset > at { offset + length } else { offset },
        )
    }
}

pub(super) fn buffer_matches(buffer: &Buffer, text: &str) -> bool {
    buffer.lines.len() == 1 && buffer.lines[0].text() == text
}

pub(super) fn key(position: Position) -> (usize, usize) {
    (position.row, position.offset)
}
pub(super) fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}
pub(super) fn boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}
