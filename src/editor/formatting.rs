//! Selection formatting and the caret's independently toggled insertion style.
use super::Editor;
use crate::model::bold;

impl Editor {
    /// Insertion weight can differ from a mixed selection's button state.
    pub fn input_bold(&self) -> bool {
        self.typing_bold
    }

    pub fn bold_active(&self) -> bool {
        if self.anchor == self.focus {
            return self.typing_bold;
        }
        let start = self.anchor.min(self.focus);
        let end = self.anchor.max(self.focus);
        let mut has_text = false;
        for row in start.row..=end.row {
            let from = if row == start.row { start.offset } else { 0 };
            let to = if row == end.row {
                end.offset
            } else {
                self.row_text(row).len()
            };
            if from < to {
                has_text = true;
                if !bold::covers(self.row_bold_at(&self.row_locations[row]), from, to) {
                    return false;
                }
            }
        }
        has_text
    }

    /// A caret-only toggle changes transient input style, not stored content.
    pub fn toggle_bold(&mut self) -> bool {
        self.typing = None;
        if self.anchor == self.focus {
            self.typing_bold = !self.typing_bold;
            return false;
        }
        let enabled = !self.bold_active();
        let start = self.anchor.min(self.focus);
        let end = self.anchor.max(self.focus);
        let has_text = (start.row..=end.row).any(|row| {
            let from = if row == start.row { start.offset } else { 0 };
            let to = if row == end.row {
                end.offset
            } else {
                self.row_text(row).len()
            };
            from < to
        });
        if !has_text {
            return false;
        }
        let before = self.snapshot();
        for row in start.row..=end.row {
            let from = if row == start.row { start.offset } else { 0 };
            let to = if row == end.row {
                end.offset
            } else {
                self.row_text(row).len()
            };
            if from < to {
                let (_, ranges) = self.row_parts_mut(row);
                bold::set(ranges, from, to, enabled);
            }
        }
        self.typing_bold = enabled;
        self.revision = self.revision.saturating_add(1);
        self.clear_redo();
        self.push_undo(before);
        true
    }

    pub(super) fn refresh_typing_bold(&mut self) {
        let offset = self.focus.offset.saturating_sub(1);
        self.typing_bold = self
            .row_bold_at(&self.row_locations[self.focus.row])
            .iter()
            .any(|range| range.start <= offset && offset < range.end);
    }
}
