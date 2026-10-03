//! Inline formatting commands shared by title-bar actions and shortcuts.

use super::*;

impl App {
    pub(super) fn toggle_bold(&mut self, id: &str) -> Result<()> {
        let Some(panel) = self.notes.get_mut(id) else {
            return Ok(());
        };
        if panel.ui.get_frozen()
            || !panel.ui.get_error().is_empty()
            || !panel.preedit.is_empty()
            || panel.waiting_ime
        {
            return Ok(());
        }
        let Some(editor) = &mut panel.editor else {
            return Ok(());
        };
        if editor.toggle_bold() {
            panel.changed();
            self.list_dirty = true;
            self.list_dirty_rows.insert(id.to_owned());
        }
        // An empty selection changes only the next insertion's style, not the
        // document revision, saved timestamps or a new draft's persistence.
        panel.render_dirty = true;
        panel.follow_caret = true;
        panel.ui.invoke_focus_editor();
        Ok(())
    }
}
