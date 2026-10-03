//! Bounded undo/redo snapshots.
use super::{Editor, HISTORY_BYTES, HISTORY_LIMIT, Snapshot};

impl Editor {
    pub fn undo(&mut self) -> bool {
        self.typing = None;
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.history_bytes = self.history_bytes.saturating_sub(previous.bytes);
        let current = self.restore(previous);
        self.history_bytes = self.history_bytes.saturating_add(current.bytes);
        self.redo.push(current);
        self.trim_history();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.typing = None;
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.history_bytes = self.history_bytes.saturating_sub(next.bytes);
        let current = self.restore(next);
        self.push_undo(current);
        true
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            document: self.document.clone(),
            anchor: self.anchor,
            focus: self.focus,
            bytes: self.document.estimated_size(),
            typing_bold: self.typing_bold,
        }
    }

    pub(super) fn restore(&mut self, snapshot: Snapshot) -> Snapshot {
        let anchor = self.anchor;
        let focus = self.focus;
        let typing_bold = self.typing_bold;
        let bytes = self.document.estimated_size();
        let previous = std::mem::replace(&mut self.document, snapshot.document);
        self.index_rows();
        self.anchor = snapshot.anchor;
        self.focus = snapshot.focus;
        self.typing_bold = snapshot.typing_bold;
        self.revision = self.revision.saturating_add(1);
        Snapshot {
            document: previous,
            anchor,
            focus,
            bytes,
            typing_bold,
        }
    }

    pub(super) fn push_undo(&mut self, snapshot: Snapshot) {
        self.history_bytes = self.history_bytes.saturating_add(snapshot.bytes);
        self.undo.push_back(snapshot);
        self.trim_history();
    }

    pub(super) fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > 1
            && (self.undo.len() + self.redo.len() > HISTORY_LIMIT
                || self.history_bytes > HISTORY_BYTES)
        {
            if let Some(oldest) = self.undo.pop_front() {
                self.history_bytes = self.history_bytes.saturating_sub(oldest.bytes);
            } else {
                let farthest_redo = self.redo.remove(0);
                self.history_bytes = self.history_bytes.saturating_sub(farthest_redo.bytes);
            }
        }
    }

    pub(super) fn clear_redo(&mut self) {
        for state in self.redo.drain(..) {
            self.history_bytes = self.history_bytes.saturating_sub(state.bytes);
        }
    }
}
