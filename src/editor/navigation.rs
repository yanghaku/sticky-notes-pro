//! Borrowed row indexing, selections, and grapheme-safe navigation.
use super::{Block, Editor, Position, RowLocation, Unit};
use crate::model::BoldRange;
use unicode_segmentation::UnicodeSegmentation;

impl Editor {
    pub fn selection(&self) -> (Position, Position) {
        (self.anchor, self.focus)
    }

    pub fn select(&mut self, anchor: Position, focus: Position) {
        self.anchor = self.clamp_cursor(anchor);
        self.focus = self.clamp_cursor(focus);
        self.typing = None;
        self.refresh_typing_bold();
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.anchor != self.focus {
            self.move_to(self.anchor.min(self.focus), false);
            return;
        }
        let mut cursor = self.focus;
        if cursor.offset > 0 {
            cursor.offset = previous_boundary(self.row_text(cursor.row), cursor.offset);
        } else if cursor.row > 0 {
            cursor.row -= 1;
            cursor.offset = self.row_text(cursor.row).len();
        }
        self.move_to(cursor, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && self.anchor != self.focus {
            self.move_to(self.anchor.max(self.focus), false);
            return;
        }
        let mut cursor = self.focus;
        if cursor.offset < self.row_text(cursor.row).len() {
            cursor.offset = next_boundary(self.row_text(cursor.row), cursor.offset);
        } else if cursor.row + 1 < self.row_locations.len() {
            cursor.row += 1;
            cursor.offset = 0;
        }
        self.move_to(cursor, extend);
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let column = self.row_text(self.focus.row)[..self.focus.offset]
            .graphemes(true)
            .count();
        let row = self
            .focus
            .row
            .saturating_add_signed(delta)
            .min(self.row_locations.len() - 1);
        let text = self.row_text(row);
        let offset = text
            .grapheme_indices(true)
            .nth(column)
            .map_or(text.len(), |(offset, _)| offset);
        self.move_to(Position { row, offset }, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(
            Position {
                row: self.focus.row,
                offset: 0,
            },
            extend,
        );
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(
            Position {
                row: self.focus.row,
                offset: self.row_text(self.focus.row).len(),
            },
            extend,
        );
    }

    pub fn select_all(&mut self) {
        self.anchor = Position::default();
        self.document_end(true);
    }

    pub fn document_home(&mut self, extend: bool) {
        self.move_to(Position::default(), extend);
    }

    pub fn document_end(&mut self, extend: bool) {
        let row = self.row_locations.len() - 1;
        self.move_to(
            Position {
                row,
                offset: self.row_text(row).len(),
            },
            extend,
        );
    }

    pub fn clipboard_text(&self) -> String {
        if self.anchor == self.focus {
            return String::new();
        }
        let start = self.anchor.min(self.focus);
        let end = self.anchor.max(self.focus);
        let rows = self.rows();
        let mut selected = Vec::new();
        for (row, unit) in rows.iter().enumerate().take(end.row + 1).skip(start.row) {
            let from = if row == start.row { start.offset } else { 0 };
            let to = if row == end.row {
                end.offset
            } else {
                unit.text.len()
            };
            // The next row's offset zero selects only the preceding boundary.
            if row == end.row && to == 0 && row != start.row {
                selected.push(String::new());
                break;
            }
            let mut line = String::new();
            if from == 0 {
                line.push_str(unit.marker);
            }
            line.push_str(&unit.text[from..to]);
            selected.push(line);
        }
        selected.join("\n")
    }

    pub(super) fn move_to(&mut self, cursor: Position, extend: bool) {
        self.focus = cursor;
        if !extend {
            self.anchor = cursor;
        }
        self.typing = None;
        self.refresh_typing_bold();
    }

    pub(super) fn index_rows(&mut self) {
        self.row_locations.clear();
        self.nonempty_rows = 0;
        for (block, content) in self.document.blocks.iter().enumerate() {
            match content {
                Block::Paragraph { text, .. } => {
                    let nonempty = !text.trim().is_empty();
                    self.nonempty_rows += usize::from(nonempty);
                    self.row_locations.push(RowLocation {
                        block,
                        item: None,
                        nonempty,
                    });
                }
                Block::OrderedList { items, .. } | Block::BulletList { items } => {
                    for (item, content) in items.iter().enumerate() {
                        let nonempty = !content.text.trim().is_empty();
                        self.nonempty_rows += usize::from(nonempty);
                        self.row_locations.push(RowLocation {
                            block,
                            item: Some(item),
                            nonempty,
                        });
                    }
                }
            }
        }
        // One shared numbering projection for rows, export, and previews. Built
        // only when content changes, so painting never formats a marker again.
        self.row_markers = self
            .document
            .projected_rows()
            .map(|(_, marker)| marker)
            .collect();
    }

    pub(super) fn row_text(&self, row: usize) -> &str {
        self.row_text_at(&self.row_locations[row])
    }

    pub(super) fn row_parts_mut(&mut self, row: usize) -> (&mut String, &mut Vec<BoldRange>) {
        let location = self.row_locations[row];
        match &mut self.document.blocks[location.block] {
            Block::Paragraph { text, bold } => (text, bold),
            Block::OrderedList { items, .. } | Block::BulletList { items } => {
                let item = &mut items[location.item.expect("list row has an item index")];
                (&mut item.text, &mut item.bold)
            }
        }
    }

    pub(super) fn clamp_cursor(&self, cursor: Position) -> Position {
        let row = cursor.row.min(self.row_locations.len() - 1);
        Position {
            row,
            offset: clamp_offset(self.row_text(row), cursor.offset),
        }
    }
}

pub(super) fn clamp_position(units: &[Unit], position: Position) -> Position {
    let row = position.row.min(units.len() - 1);
    let text = &units[row].text;
    let offset = clamp_offset(text, position.offset);
    Position { row, offset }
}

pub(super) fn clamp_offset(text: &str, offset: usize) -> usize {
    let target = offset.min(text.len());
    if target == text.len() {
        target
    } else {
        text.grapheme_indices(true)
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset <= target)
            .last()
            .unwrap_or(0)
    }
}

pub(super) fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .take_while(|index| *index < offset)
        .last()
        .unwrap_or(0)
}

pub(super) fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .find(|index| *index > offset)
        .unwrap_or(text.len())
}
