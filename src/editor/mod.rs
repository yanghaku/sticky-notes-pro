//! Editor commands own document changes, selections, numbering, and history.

use std::{collections::VecDeque, time::Instant};

use crate::model::{Block, Document};

mod history;
mod input;
mod navigation;
mod structure;
#[cfg(test)]
mod tests;

const HISTORY_LIMIT: usize = 100;
const HISTORY_BYTES: usize = 32 * 1024 * 1024;

/// A logical text unit and UTF-8 byte offset, excluding its generated marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRow<'a> {
    pub text: &'a str,
    pub marker: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Paragraph,
    Ordered(usize),
    Bullet(usize),
}

impl Kind {
    fn is_list(self) -> bool {
        matches!(self, Self::Ordered(_) | Self::Bullet(_))
    }

    fn same_list_type(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Ordered(_), Self::Ordered(_)) | (Self::Bullet(_), Self::Bullet(_))
        )
    }
}

struct Unit {
    text: String,
    kind: Kind,
}

#[derive(Clone)]
struct Snapshot {
    document: Document,
    anchor: Position,
    focus: Position,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct RowLocation {
    block: usize,
    item: Option<usize>,
    nonempty: bool,
}

pub struct Editor {
    document: Document,
    anchor: Position,
    focus: Position,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    typing: Option<(Instant, Position)>,
    revision: u64,
    row_locations: Vec<RowLocation>,
    row_markers: Vec<String>,
    nonempty_rows: usize,
    history_bytes: usize,
    empty_recovery: Option<Document>,
}

impl Editor {
    pub fn new(mut document: Document) -> Self {
        // Database parsing validates first. Normalization also makes an empty
        // in-memory document safe to edit without losing unknown formats.
        let empty = document.is_empty();
        document
            .blocks
            .retain(|block| !matches!(block, Block::OrderedList { items, .. } | Block::BulletList { items } if items.is_empty()));
        if document.blocks.is_empty() {
            document.blocks.push(Block::Paragraph {
                text: String::new(),
            });
        }
        let mut editor = Self {
            document,
            anchor: Position::default(),
            focus: Position::default(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            typing: None,
            revision: 0,
            row_locations: Vec::new(),
            row_markers: Vec::new(),
            nonempty_rows: 0,
            history_bytes: 0,
            empty_recovery: None,
        };
        editor.index_rows();
        debug_assert_eq!(editor.is_empty(), empty);
        editor
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Transfer the exact pre-clear state, including unsaved edits, to its owner.
    /// Captured only at a nonempty-to-empty transition, not on ordinary typing.
    pub fn take_empty_recovery(&mut self) -> Option<Document> {
        self.empty_recovery.take()
    }

    /// Recovery content must never outlive the empty document it describes.
    pub(super) fn clear_empty_recovery(&mut self) {
        if !self.is_empty() {
            self.empty_recovery = None;
        }
    }

    /// Record the transition into an empty document. `previous` is only built
    /// when it will actually be kept, so undo and redo avoid a needless copy.
    pub(super) fn track_empty_recovery(
        &mut self,
        was_nonempty: bool,
        previous: impl FnOnce() -> Document,
    ) {
        if !self.is_empty() {
            self.clear_empty_recovery();
        } else if was_nonempty {
            self.empty_recovery = Some(previous());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nonempty_rows == 0
    }

    pub fn rows(&self) -> Vec<EditorRow<'_>> {
        // Numbering is generated when the document changes (index_rows), so each
        // paint or copy only borrows it instead of reformatting every marker.
        debug_assert_eq!(self.row_locations.len(), self.row_markers.len());
        self.row_locations
            .iter()
            .zip(&self.row_markers)
            .map(|(location, marker)| EditorRow {
                text: self.row_text_at(location),
                marker,
            })
            .collect()
    }

    fn row_text_at(&self, location: &RowLocation) -> &str {
        match &self.document.blocks[location.block] {
            Block::Paragraph { text } => text,
            Block::OrderedList { items, .. } | Block::BulletList { items } => {
                &items[location.item.expect("list rows have an item")].text
            }
        }
    }
}
