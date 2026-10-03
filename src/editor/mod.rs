//! Editor commands own document changes, selections, numbering, and history.

use std::{collections::VecDeque, time::Instant};

use crate::model::{Block, BoldRange, Document};

mod formatting;
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
    pub bold: &'a [BoldRange],
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
    bold: Vec<BoldRange>,
    kind: Kind,
}

#[derive(Clone)]
struct Snapshot {
    document: Document,
    anchor: Position,
    focus: Position,
    bytes: usize,
    typing_bold: bool,
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
    typing_bold: bool,
    revision: u64,
    row_locations: Vec<RowLocation>,
    row_markers: Vec<String>,
    nonempty_rows: usize,
    history_bytes: usize,
}

impl Editor {
    pub fn new(mut document: Document) -> Self {
        // Database parsing validates first. Normalization also makes an empty
        // in-memory document safe to edit without losing unknown formats.
        document.normalize_bold();
        let empty = document.is_empty();
        document
            .blocks
            .retain(|block| !matches!(block, Block::OrderedList { items, .. } | Block::BulletList { items } if items.is_empty()));
        if document.blocks.is_empty() {
            document.blocks.push(Block::Paragraph {
                text: String::new(),
                bold: Vec::new(),
            });
        }
        let mut editor = Self {
            document,
            anchor: Position::default(),
            focus: Position::default(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            typing: None,
            typing_bold: false,
            revision: 0,
            row_locations: Vec::new(),
            row_markers: Vec::new(),
            nonempty_rows: 0,
            history_bytes: 0,
        };
        editor.index_rows();
        editor.refresh_typing_bold();
        debug_assert_eq!(editor.is_empty(), empty);
        editor
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn revision(&self) -> u64 {
        self.revision
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
                bold: self.row_bold_at(location),
            })
            .collect()
    }

    fn row_text_at(&self, location: &RowLocation) -> &str {
        match &self.document.blocks[location.block] {
            Block::Paragraph { text, .. } => text,
            Block::OrderedList { items, .. } | Block::BulletList { items } => {
                &items[location.item.expect("list rows have an item")].text
            }
        }
    }

    fn row_bold_at(&self, location: &RowLocation) -> &[BoldRange] {
        match &self.document.blocks[location.block] {
            Block::Paragraph { bold, .. } => bold,
            Block::OrderedList { items, .. } | Block::BulletList { items } => {
                &items[location.item.expect("list rows have an item")].bold
            }
        }
    }
}
