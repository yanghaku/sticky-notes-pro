//! Text insertion, IME commits, and fast single-row edits.
use super::navigation::{clamp_offset, next_boundary};
use super::structure::{delete_selection, insert_units};
use super::{Editor, Kind, Position};
use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

impl Editor {
    /// `autocomplete` is true for direct typing, false for paste.
    pub fn insert_text(&mut self, text: &str, autocomplete: bool) -> bool {
        let normalized = if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        };
        if normalized.is_empty() && self.anchor == self.focus {
            return false;
        }
        let had_selection = self.anchor != self.focus;
        let now = Instant::now();
        let coalesce = autocomplete
            && !had_selection
            && !normalized.contains('\n')
            && self.typing.is_some_and(|(when, end)| {
                end == self.focus && now.duration_since(when) <= Duration::from_secs(1)
            });
        let before = (!coalesce).then(|| self.snapshot());
        if !had_selection && !normalized.contains('\n') {
            let inserted = if !autocomplete && self.row_locations[self.focus.row].item.is_some() {
                strip_copied_markers(&normalized)
            } else {
                Cow::Borrowed(normalized.as_ref())
            };
            if inserted.is_empty() {
                self.typing = None;
                return false;
            }
            self.insert_single_line(&inserted);
        } else {
            let mut units = self.units();
            let cursor = delete_selection(&mut units, self.anchor, self.focus);
            // Deleting a selection can replace the list with a paragraph. Only
            // strip generated markers when the actual insertion target is a list.
            let inserted = if !autocomplete && units[cursor.row].kind.is_list() {
                strip_copied_markers(&normalized)
            } else {
                Cow::Borrowed(normalized.as_ref())
            };
            let cursor = insert_units(&mut units, cursor, &inserted);
            if !self.install(units, cursor) {
                // Content is unchanged, but the caret collapsed out of its
                // selection: end the typing burst without recording history.
                self.typing = None;
                return false;
            }
        }
        self.clear_redo();
        if let Some(before) = before {
            self.push_undo(before);
        }
        let mut cursor = self.focus;
        let conversion = autocomplete
            && !had_selection
            && normalized.ends_with(' ')
            && !normalized.contains('\n')
            && matches!(cursor.offset, 2 | 3);
        // Inspect the borrowed row before taking the structural slow path.
        let prefix = conversion
            && self.row_locations[cursor.row].item.is_none()
            && matches!(
                self.row_text(cursor.row).get(..cursor.offset),
                Some("1. " | "- " | "* ")
            );
        if prefix {
            let mut units = self.units();
            let group = units.len();
            let unit = &mut units[cursor.row];
            let prefix = match unit.text.get(..cursor.offset) {
                Some("1. ") => Some((3, Kind::Ordered(group))),
                Some("- " | "* ") => Some((2, Kind::Bullet(group))),
                _ => None,
            };
            if unit.kind == Kind::Paragraph
                && let Some((length, kind)) = prefix
            {
                self.push_undo(self.snapshot());
                unit.text.drain(..length);
                unit.kind = kind;
                cursor.offset = 0;
                self.install(units, cursor);
                self.typing = None;
                return true;
            }
        }
        self.typing = if autocomplete && !had_selection && !normalized.contains('\n') {
            Some((now, self.focus))
        } else {
            None
        };
        true
    }

    /// An IME commit is one isolated history unit. Preedit stays outside Document.
    pub fn commit_text(&mut self, text: &str) -> bool {
        self.typing = None;
        let changed = self.insert_text(text, true);
        self.typing = None;
        changed
    }

    // Insertion cannot erase existing non-whitespace content. This updates the
    // empty-note cache using only the inserted string, not the whole document.
    pub(super) fn insert_single_line(&mut self, text: &str) {
        let mut cursor = self.focus;
        let body = self.row_text_mut(cursor.row);
        if cursor.offset == body.len() {
            body.push_str(text);
        } else {
            body.insert_str(cursor.offset, text);
        }
        cursor.offset += text.len();
        let clamped = clamp_offset(body, cursor.offset);
        if clamped < cursor.offset {
            cursor.offset = next_boundary(body, clamped);
        }
        let location = &mut self.row_locations[cursor.row];
        if !location.nonempty && !text.trim().is_empty() {
            location.nonempty = true;
            self.nonempty_rows += 1;
        }
        self.clear_empty_recovery();
        self.anchor = cursor;
        self.focus = cursor;
        self.revision = self.revision.saturating_add(1);
    }

    pub(super) fn delete_in_row(&mut self, from: usize, to: usize) -> bool {
        let before = self.snapshot();
        let was_nonempty = !self.is_empty();
        let row = self.focus.row;
        let text = self.row_text_mut(row);
        text.drain(from..to);
        let mut offset = clamp_offset(text, from);
        if offset < from {
            offset = next_boundary(text, offset);
        }
        let nonempty = !text.trim().is_empty();
        self.nonempty_rows = self.nonempty_rows - usize::from(self.row_locations[row].nonempty)
            + usize::from(nonempty);
        self.row_locations[row].nonempty = nonempty;
        self.track_empty_recovery(was_nonempty, || before.document.clone());
        self.anchor = Position { row, offset };
        self.focus = self.anchor;
        self.typing = None;
        self.revision = self.revision.saturating_add(1);
        self.clear_redo();
        self.push_undo(before);
        true
    }
}

/// Drop a generated marker (`12. ` or `• `) that `clipboard_text` wrote at the
/// start of each line. Any other leading text is left alone.
fn strip_copied_markers(text: &str) -> Cow<'_, str> {
    if !text.split('\n').any(|line| marker_len(line) > 0) {
        return Cow::Borrowed(text);
    }
    let mut stripped = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            stripped.push('\n');
        }
        stripped.push_str(&line[marker_len(line)..]);
    }
    Cow::Owned(stripped)
}

fn marker_len(line: &str) -> usize {
    let digits = line
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits > 0 && line.as_bytes().get(digits..digits + 2) == Some(b". ") {
        return digits + 2;
    }
    if line.starts_with("• ") {
        return "• ".len();
    }
    0
}
