//! Editable window state transitions.

use super::*;

impl Panel {
    pub(super) fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.last_edit = Instant::now();
        if let Some(note) = &mut self.note {
            note.updated_at = now_millis().max(note.updated_at.saturating_add(1));
        }
        self.dirty_since.get_or_insert(self.last_edit);
        self.ui.set_status("未保存".into());
        self.render_dirty = true;
        self.follow_caret = true;
    }
}
