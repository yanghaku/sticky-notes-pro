//! Versioned, structured note content. Display markers are derived, never stored.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub(crate) mod bold;
pub use bold::BoldRange;

pub const DOCUMENT_FORMAT_VERSION: u32 = 1;

/// Note colors, in the order the settings panel lists them. Storage validation
/// and the panel's picker must agree on this single set.
pub const THEMES: [&str; 7] = ["white", "yellow", "green", "pink", "purple", "blue", "gray"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub format_version: u32,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Paragraph {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bold: Vec<BoldRange>,
    },
    OrderedList {
        items: Vec<ListItem>,
        /// Continue the preceding ordered list across ordinary paragraphs.
        #[serde(default, skip_serializing_if = "is_false")]
        continuation: bool,
    },
    BulletList {
        items: Vec<ListItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListItem {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bold: Vec<BoldRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub document: Document,
    pub theme_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format_version: DOCUMENT_FORMAT_VERSION,
            blocks: vec![Block::Paragraph {
                text: String::new(),
                bold: Vec::new(),
            }],
        }
    }
}

impl Document {
    /// Decode supported content without silently replacing corrupt data.
    pub fn from_json(json: &str) -> Result<Self> {
        let mut document: Self = serde_json::from_str(json).context("invalid document JSON")?;
        document.validate()?;
        document.normalize_bold();
        Ok(document)
    }

    /// Accept old v1 character-aligned ranges without retaining partial graphemes.
    pub(crate) fn normalize_bold(&mut self) {
        for block in &mut self.blocks {
            match block {
                Block::Paragraph { text, bold: ranges } => bold::normalize(text, ranges),
                Block::OrderedList { items, .. } | Block::BulletList { items } => {
                    for item in items {
                        bold::normalize(&item.text, &mut item.bold);
                    }
                }
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format_version == DOCUMENT_FORMAT_VERSION,
            "unsupported document format version {}",
            self.format_version
        );
        ensure!(!self.blocks.is_empty(), "document must contain a text unit");
        let mut has_ordered_list = false;
        for block in &self.blocks {
            match block {
                Block::Paragraph { text, bold } => {
                    validate_text(text)?;
                    bold::validate(text, bold)?;
                }
                Block::OrderedList {
                    items,
                    continuation,
                } => {
                    ensure!(
                        !*continuation || has_ordered_list,
                        "ordered continuation has no preceding ordered list"
                    );
                    has_ordered_list = true;
                    ensure!(!items.is_empty(), "list must contain an item");
                    for item in items {
                        validate_text(&item.text)?;
                        bold::validate(&item.text, &item.bold)?;
                    }
                }
                Block::BulletList { items } => {
                    has_ordered_list = false;
                    ensure!(!items.is_empty(), "list must contain an item");
                    for item in items {
                        validate_text(&item.text)?;
                        bold::validate(&item.text, &item.bold)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Search projection; deliberately excludes generated list markers.
    pub fn plain_text(&self) -> String {
        self.project(false)
    }

    /// Preview and whole-document export projection, including list numbering.
    pub fn display_text(&self) -> String {
        self.project(true)
    }

    /// A bounded display prefix. The Unicode character budget includes list
    /// markers and newlines. CRLF/CR are normalized without scanning the tail.
    pub fn preview_text(&self, max_lines: usize, max_chars: usize) -> String {
        if max_lines == 0 || max_chars == 0 {
            return String::new();
        }
        let mut preview = String::with_capacity(max_chars.min(256));
        let mut remaining_chars = max_chars;
        let mut remaining_newlines = max_lines - 1;
        for (row, (text, marker)) in self.projected_rows().enumerate() {
            if row != 0 {
                if remaining_newlines == 0 || remaining_chars == 0 {
                    return preview;
                }
                preview.push('\n');
                remaining_newlines -= 1;
                remaining_chars -= 1;
            }
            let mut chars = marker.chars().chain(text.chars()).peekable();
            while remaining_chars != 0 {
                let Some(mut ch) = chars.next() else {
                    break;
                };
                if ch == '\r' {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    ch = '\n';
                }
                if ch == '\n' {
                    if remaining_newlines == 0 {
                        return preview;
                    }
                    remaining_newlines -= 1;
                }
                preview.push(ch);
                remaining_chars -= 1;
            }
            if remaining_chars == 0 || remaining_newlines == 0 {
                return preview;
            }
        }
        preview
    }

    /// Generated markers and whitespace alone do not make a note nonempty.
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|block| match block {
            Block::Paragraph { text, .. } => text.trim().is_empty(),
            Block::OrderedList { items, .. } | Block::BulletList { items } => {
                items.iter().all(|item| item.text.trim().is_empty())
            }
        })
    }

    fn project(&self, markers: bool) -> String {
        let mut projection = String::new();
        for (row, (text, marker)) in self.projected_rows().enumerate() {
            if row != 0 {
                projection.push('\n');
            }
            if markers {
                projection.push_str(&marker);
            }
            projection.push_str(text);
        }
        projection
    }

    /// One shared numbering projection for the editor, export, and previews.
    pub(crate) fn projected_rows(&self) -> impl Iterator<Item = (&str, String)> {
        let mut blocks = self.blocks.iter();
        let mut items: Option<std::slice::Iter<'_, ListItem>> = None;
        let mut ordered_count = 0usize;
        let mut bullet = false;
        std::iter::from_fn(move || {
            loop {
                if let Some(item) = items.as_mut().and_then(Iterator::next) {
                    let marker = if bullet {
                        "• ".into()
                    } else {
                        ordered_count = ordered_count.saturating_add(1);
                        format!("{ordered_count}. ")
                    };
                    return Some((item.text.as_str(), marker));
                }
                let block = blocks.next()?;
                match block {
                    Block::Paragraph { text, .. } => return Some((text.as_str(), String::new())),
                    Block::OrderedList {
                        items: list_items,
                        continuation,
                    } => {
                        if !continuation {
                            ordered_count = 0;
                        }
                        bullet = false;
                        items = Some(list_items.iter());
                    }
                    Block::BulletList { items: list_items } => {
                        ordered_count = 0;
                        bullet = true;
                        items = Some(list_items.iter());
                    }
                }
            }
        })
    }

    pub(crate) fn estimated_size(&self) -> usize {
        self.blocks
            .iter()
            .fold(std::mem::size_of::<Self>(), |total, block| {
                total
                    .saturating_add(std::mem::size_of::<Block>())
                    .saturating_add(match block {
                        Block::Paragraph { text, bold } => text.len().saturating_add(
                            bold.len().saturating_mul(std::mem::size_of::<BoldRange>()),
                        ),
                        Block::OrderedList { items, .. } | Block::BulletList { items } => {
                            items.iter().fold(0usize, |size, item| {
                                size.saturating_add(std::mem::size_of::<ListItem>())
                                    .saturating_add(item.text.len())
                                    .saturating_add(
                                        item.bold
                                            .len()
                                            .saturating_mul(std::mem::size_of::<BoldRange>()),
                                    )
                            })
                        }
                    })
            })
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

fn validate_text(text: &str) -> Result<()> {
    ensure!(
        !text.contains(['\r', '\n']),
        "text unit contains a structural newline"
    );
    Ok(())
}
