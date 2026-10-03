//! Structured list operations and cross-row edits.
use super::navigation::{clamp_position, next_boundary, previous_boundary};
use super::{Editor, Kind, Position, Snapshot, Unit};
use crate::model::bold;
use crate::model::{Block, DOCUMENT_FORMAT_VERSION, Document, ListItem};

impl Editor {
    pub fn enter(&mut self) -> bool {
        let before = self.snapshot();
        let mut units = self.units();
        let mut cursor = delete_selection(&mut units, self.anchor, self.focus);
        if units[cursor.row].kind.is_list() && units[cursor.row].text.is_empty() {
            units[cursor.row].kind = Kind::Paragraph;
        } else {
            cursor = insert_units(&mut units, cursor, "\n", self.typing_bold);
        }
        self.finish_command(before, units, cursor)
    }

    pub fn backspace(&mut self) -> bool {
        if self.anchor == self.focus && self.focus.offset > 0 {
            let previous = previous_boundary(self.row_text(self.focus.row), self.focus.offset);
            return self.delete_in_row(previous, self.focus.offset);
        }
        let before = self.snapshot();
        let mut units = self.units();
        let mut cursor = self.focus;
        if self.anchor != self.focus {
            cursor = delete_selection(&mut units, self.anchor, self.focus);
        } else if cursor.offset > 0 {
            let previous = previous_boundary(&units[cursor.row].text, cursor.offset);
            units[cursor.row].text.drain(previous..cursor.offset);
            bold::edit(
                &mut units[cursor.row].bold,
                previous,
                cursor.offset,
                0,
                false,
            );
            cursor.offset = previous;
        } else if units[cursor.row].kind.is_list() {
            units[cursor.row].kind = Kind::Paragraph;
        } else if cursor.row > 0 {
            let current = units.remove(cursor.row);
            cursor.row -= 1;
            cursor.offset = units[cursor.row].text.len();
            units[cursor.row].text.push_str(&current.text);
            bold::append(&mut units[cursor.row].bold, &current.bold, cursor.offset);
            join_following_list(&mut units, cursor.row);
        }
        let changed = self.finish_command(before, units, cursor);
        self.refresh_typing_bold();
        changed
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.anchor == self.focus && self.focus.offset < self.row_text(self.focus.row).len() {
            let next = next_boundary(self.row_text(self.focus.row), self.focus.offset);
            return self.delete_in_row(self.focus.offset, next);
        }
        let before = self.snapshot();
        let mut units = self.units();
        let mut cursor = self.focus;
        if self.anchor != self.focus {
            cursor = delete_selection(&mut units, self.anchor, self.focus);
        } else if cursor.offset < units[cursor.row].text.len() {
            let next = next_boundary(&units[cursor.row].text, cursor.offset);
            units[cursor.row].text.drain(cursor.offset..next);
            bold::edit(&mut units[cursor.row].bold, cursor.offset, next, 0, false);
        } else if cursor.row + 1 < units.len() {
            let next = units.remove(cursor.row + 1);
            units[cursor.row].text.push_str(&next.text);
            bold::append(&mut units[cursor.row].bold, &next.bold, cursor.offset);
            if next.kind == Kind::Paragraph {
                join_following_list(&mut units, cursor.row);
            } else {
                join_list_group(&mut units, cursor.row, next.kind);
            }
        }
        let changed = self.finish_command(before, units, cursor);
        self.refresh_typing_bold();
        changed
    }

    pub(super) fn units(&self) -> Vec<Unit> {
        let mut units = Vec::new();
        let mut ordered_group = None;
        for (index, block) in self.document.blocks.iter().enumerate() {
            match block {
                Block::Paragraph { text, bold } => units.push(Unit {
                    text: text.clone(),
                    bold: bold.clone(),
                    kind: Kind::Paragraph,
                }),
                Block::OrderedList {
                    items,
                    continuation,
                } => {
                    let group = if *continuation {
                        ordered_group.unwrap_or(index)
                    } else {
                        index
                    };
                    ordered_group = Some(group);
                    units.extend(items.iter().map(|item| Unit {
                        text: item.text.clone(),
                        bold: item.bold.clone(),
                        kind: Kind::Ordered(group),
                    }));
                }
                Block::BulletList { items } => {
                    ordered_group = None;
                    units.extend(items.iter().map(|item| Unit {
                        text: item.text.clone(),
                        bold: item.bold.clone(),
                        kind: Kind::Bullet(index),
                    }));
                }
            }
        }
        units
    }

    /// Rebuild the document from edited units and place the caret.
    ///
    /// The return value means "content changed" and nothing else: a no-op edit
    /// (for example typing text identical to the selection) still collapses the
    /// caret into `cursor` while returning false. Callers must re-render such a
    /// result but must not record history or bump the revision for it.
    pub(super) fn install(&mut self, units: Vec<Unit>, cursor: Position) -> bool {
        // Insertion can join combining marks across the insertion boundary.
        // Keep the resulting caret outside the newly formed grapheme cluster.
        let clamped = clamp_position(&units, cursor);
        let cursor = if clamped.row == cursor.row && clamped.offset < cursor.offset {
            Position {
                row: clamped.row,
                offset: next_boundary(&units[clamped.row].text, clamped.offset),
            }
        } else {
            clamped
        };
        let mut blocks = Vec::new();
        let mut previous = None;
        let mut ordered_group = None;
        for mut unit in units {
            bold::normalize(&unit.text, &mut unit.bold);
            match unit.kind {
                Kind::Paragraph => blocks.push(Block::Paragraph {
                    text: unit.text,
                    bold: unit.bold,
                }),
                Kind::Ordered(group) => {
                    if previous == Some(Kind::Ordered(group)) {
                        if let Some(Block::OrderedList { items, .. }) = blocks.last_mut() {
                            items.push(ListItem {
                                text: unit.text,
                                bold: unit.bold,
                            });
                        }
                    } else {
                        blocks.push(Block::OrderedList {
                            items: vec![ListItem {
                                text: unit.text,
                                bold: unit.bold,
                            }],
                            continuation: ordered_group == Some(group),
                        });
                    }
                    ordered_group = Some(group);
                }
                Kind::Bullet(group) => {
                    ordered_group = None;
                    if previous == Some(Kind::Bullet(group)) {
                        if let Some(Block::BulletList { items }) = blocks.last_mut() {
                            items.push(ListItem {
                                text: unit.text,
                                bold: unit.bold,
                            });
                        }
                    } else {
                        blocks.push(Block::BulletList {
                            items: vec![ListItem {
                                text: unit.text,
                                bold: unit.bold,
                            }],
                        });
                    }
                }
            }
            previous = Some(unit.kind);
        }
        if blocks.is_empty() {
            blocks.push(Block::Paragraph {
                text: String::new(),
                bold: Vec::new(),
            });
        }
        let document = Document {
            format_version: DOCUMENT_FORMAT_VERSION,
            blocks,
        };
        if document == self.document {
            // Identical content: the row index is still valid, so only move the
            // caret. Rebuilding it here would cost a full reindex per no-op edit.
            self.anchor = cursor;
            self.focus = cursor;
            return false;
        }
        self.revision = self.revision.saturating_add(1);
        self.document = document;
        self.index_rows();
        self.anchor = cursor;
        self.focus = cursor;
        true
    }

    pub(super) fn finish_command(
        &mut self,
        before: Snapshot,
        units: Vec<Unit>,
        cursor: Position,
    ) -> bool {
        self.typing = None;
        if !self.install(units, cursor) {
            return false;
        }
        self.clear_redo();
        self.push_undo(before);
        true
    }
}

pub(super) fn delete_selection(
    units: &mut Vec<Unit>,
    anchor: Position,
    focus: Position,
) -> Position {
    let start = anchor.min(focus);
    let end = anchor.max(focus);
    if start == end {
        return start;
    }
    let last = units.len() - 1;
    if start == Position::default()
        && end
            == (Position {
                row: last,
                offset: units[last].text.len(),
            })
    {
        units.clear();
        units.push(Unit {
            text: String::new(),
            bold: Vec::new(),
            kind: Kind::Paragraph,
        });
        return Position::default();
    }
    if start.row == end.row {
        units[start.row].text.drain(start.offset..end.offset);
        bold::edit(
            &mut units[start.row].bold,
            start.offset,
            end.offset,
            0,
            false,
        );
    } else if start.offset == 0
        && end.offset == 0
        && units[start.row].kind.is_list()
        && units[end.row].kind.is_list()
    {
        let previous_same_list =
            start.row > 0 && units[start.row - 1].kind == units[start.row].kind;
        units.drain(start.row..end.row);
        if previous_same_list {
            join_following_list(units, start.row - 1);
        }
    } else {
        let end_kind = units[end.row].kind;
        let suffix = units[end.row].text[end.offset..].to_owned();
        let suffix_bold =
            bold::clipped(&units[end.row].bold, end.offset, units[end.row].text.len());
        units[start.row].bold = bold::clipped(&units[start.row].bold, 0, start.offset);
        bold::append(&mut units[start.row].bold, &suffix_bold, start.offset);
        units[start.row].text.truncate(start.offset);
        units[start.row].text.push_str(&suffix);
        units.drain(start.row + 1..=end.row);
        if end_kind == Kind::Paragraph {
            join_following_list(units, start.row);
        } else {
            join_list_group(units, start.row, end_kind);
        }
    }
    start
}

pub(super) fn join_following_list(units: &mut [Unit], row: usize) {
    let Some(next) = units.get(row + 1).map(|unit| unit.kind) else {
        return;
    };
    join_list_group(units, row, next);
}

// Only an explicitly removed list/paragraph boundary joins list identities.
// Independent neighboring list blocks keep their own numbering otherwise.
pub(super) fn join_list_group(units: &mut [Unit], row: usize, group: Kind) {
    let Some(current) = units.get(row).map(|unit| unit.kind) else {
        return;
    };
    if current == group || !current.same_list_type(group) {
        return;
    }
    // The removed item may be the last item before an ordinary paragraph.
    // Join its whole identity, including later ordered continuations, rather
    // than only the adjacent run. Unrelated list identities remain untouched.
    for unit in units
        .iter_mut()
        .skip(row + 1)
        .filter(|unit| unit.kind == group)
    {
        unit.kind = current;
    }
}

pub(super) fn insert_units(
    units: &mut Vec<Unit>,
    cursor: Position,
    text: &str,
    typing_bold: bool,
) -> Position {
    let mut parts = text.split('\n');
    let first = parts.next().unwrap_or_default();
    let suffix = units[cursor.row].text.split_off(cursor.offset);
    let suffix_bold = bold::clipped(
        &units[cursor.row].bold,
        cursor.offset,
        cursor.offset + suffix.len(),
    );
    units[cursor.row].bold = bold::clipped(&units[cursor.row].bold, 0, cursor.offset);
    if typing_bold {
        bold::push(
            &mut units[cursor.row].bold,
            crate::model::BoldRange {
                start: cursor.offset,
                end: cursor.offset + first.len(),
            },
        );
    }
    units[cursor.row].text.push_str(first);
    let mut row = cursor.row;
    let kind = units[row].kind;
    for part in parts {
        row += 1;
        units.insert(
            row,
            Unit {
                text: part.to_owned(),
                bold: if typing_bold && !part.is_empty() {
                    vec![crate::model::BoldRange {
                        start: 0,
                        end: part.len(),
                    }]
                } else {
                    Vec::new()
                },
                kind,
            },
        );
    }
    let offset = units[row].text.len();
    units[row].text.push_str(&suffix);
    bold::append(&mut units[row].bold, &suffix_bold, offset);
    for (index, unit) in units.iter_mut().enumerate().take(row + 1).skip(cursor.row) {
        let from = if index == cursor.row {
            cursor.offset
        } else {
            0
        };
        let to = if index == row {
            offset
        } else {
            unit.text.len()
        };
        bold::finish_insertion(&unit.text, &mut unit.bold, from, to, typing_bold);
    }
    Position { row, offset }
}
