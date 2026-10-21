//! Periodic save coordination and window restoration.

use super::*;

impl App {
    pub(super) fn tick(&mut self, shared: &Rc<RefCell<Self>>) -> Result<()> {
        self.sync_appearance();
        self.process_replies()?;
        if self.instance.take_activation()? {
            self.open_list(shared)?;
        }
        let now = Instant::now();
        if let Some(state) = self.restore_list.take()
            && let Some(ui) = &self.list
        {
            platform::restore(ui.window(), &state);
            if let Some(tracked) = &mut self.list_state {
                tracked.restoring(now);
            }
        }
        if let Some(state) = self.restore_settings.take()
            && let Some(ui) = &self.settings_ui
        {
            platform::restore(ui.window(), &state);
            if let Some(tracked) = &mut self.settings_state {
                tracked.restoring(now);
            }
        }
        self.maintain_auxiliary_windows(now)?;
        for (window_id, panel) in &mut self.notes {
            if let Some(capabilities) = panel.initialize_window()
                && capabilities.is_wayland
            {
                if let Some(id) = &panel.state.note_id {
                    self.list_dirty_rows.insert(id.clone());
                }
                self.list_dirty = true;
            }
            if panel
                .ime_wait_started
                .is_some_and(|start| now.duration_since(start) >= Duration::from_secs(2))
            {
                // An unresponsive IME must not silently discard an unconfirmed candidate or hang close forever.
                panel.waiting_ime = false;
                panel.ime_wait_started = None;
                panel.pending_action = None;
                if let Some(id) = &panel.state.note_id {
                    self.pending_notes.remove(id);
                    self.list_dirty_rows.insert(id.clone());
                }
                self.list_dirty = true;
                panel.ui.set_frozen(false);
                panel
                    .ui
                    .set_error("输入法未结束组合，已取消关闭。请先确认或取消候选，再重试。".into());
                panel.set_ime_enabled(panel.editor.is_some());
            }
            panel.state_save_timer.observe(panel.state_dirty);
            let content_blocked = panel.in_flight
                || panel.waiting_ime
                || !panel.preedit.is_empty()
                || panel.discarding;
            let autosave = panel.dirty_since.is_some()
                && (now.duration_since(panel.last_edit) >= Duration::from_millis(400)
                    || now.duration_since(panel.last_submit) >= Duration::from_secs(2));
            // Window geometry does not depend on the IME's pending text. Keep it
            // saving during composition, but never enqueue beyond a closing or
            // destructive snapshot. A content save also carries the latest state.
            if panel.persisted
                && !panel.state_in_flight
                && panel.pending_action.is_none()
                && !panel.destructive_queued
                && !panel.discarding
                && (!autosave || content_blocked)
                && panel.state_save_timer.ready(panel.state_dirty, now)
            {
                panel.state = platform::capture(panel.ui.window(), &panel.state);
                match self.worker.submit(Job::SaveWindow {
                    state: panel.state.clone(),
                }) {
                    Ok(()) => {
                        panel.state_dirty = None;
                        panel.state_in_flight = true;
                        panel.state_save_timer.submitted();
                    }
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                    Err(error) => {
                        panel.retry_window_state(now);
                        return Err(anyhow::anyhow!("save worker unavailable: {error}"));
                    }
                }
            }
            if content_blocked || (!autosave && panel.pending_action.is_none()) {
                continue;
            }
            panel.state = platform::capture(panel.ui.window(), &panel.state);
            let action = panel.pending_action.unwrap_or(SaveAction::Save);
            let empty = panel
                .editor
                .as_ref()
                .is_some_and(|editor| editor.is_empty());
            if empty && !panel.persisted && action == SaveAction::Save {
                panel.clear_draft_identity();
                panel.dirty_since = None;
                panel.ui.set_status("".into());
                if let Some(id) = &panel.state.note_id {
                    self.list_dirty_rows.insert(id.clone());
                }
                self.list_dirty = true;
                continue;
            }
            let job = if empty && !panel.persisted {
                panel.clear_draft_identity();
                Job::CloseDraft {
                    id: window_id.clone(),
                }
            } else if panel.note.is_some() {
                panel.ensure_note_identity();
                let document = panel
                    .editor
                    .as_ref()
                    .context("editable note has no editor")?
                    .document()
                    .clone();
                Job::Save {
                    note: panel.save_snapshot(document)?,
                    state: Some(panel.state.clone()),
                    revision: panel.revision,
                    action,
                    create: !panel.persisted,
                }
            } else if action == SaveAction::Delete {
                Job::Delete {
                    id: panel
                        .state
                        .note_id
                        .clone()
                        .context("note window has no id")?,
                    state: Some(panel.state.clone()),
                }
            } else {
                Job::CloseWindow {
                    state: panel.state.clone(),
                }
            };
            match self.worker.submit(job) {
                Ok(()) => {
                    panel.in_flight = true;
                    panel.destructive_queued =
                        (empty && !panel.persisted) || action == SaveAction::Delete;
                    panel.last_submit = now;
                    panel.state_dirty = None;
                    panel.state_save_timer.submitted();
                    panel.ui.set_status("保存中".into());
                    if let Some(id) = &panel.state.note_id {
                        self.list_dirty_rows.insert(id.clone());
                    }
                    self.list_dirty = true;
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                Err(error) => {
                    panel.pending_action = None;
                    if let Some(id) = &panel.state.note_id {
                        self.pending_notes.remove(id);
                        self.list_dirty_rows.insert(id.clone());
                    }
                    self.list_dirty = true;
                    panel.ui.set_frozen(false);
                    panel
                        .ui
                        .set_error(format!("保存线程不可用：{error}").into());
                    panel.dirty_since = None;
                    panel.set_ime_enabled(panel.editor.is_some());
                }
            }
        }
        if self.list_dirty && (self.list_reload || now >= self.list_next_refresh) {
            self.refresh_list()?;
        }
        Ok(())
    }
}

impl Panel {
    /// Allocate an ID only for a nonempty first save, never when opening a draft.
    fn ensure_note_identity(&mut self) {
        if let Some(note) = &mut self.note
            && note.id.is_none()
        {
            let id = uuid::Uuid::new_v4().to_string();
            note.created_at = now_millis();
            note.updated_at = note.updated_at.max(note.created_at);
            self.state.id = format!("note:{id}");
            self.state.window_type = "note".to_owned();
            self.state.note_id = Some(id.clone());
            note.id = Some(id);
        }
    }

    pub(super) fn clear_draft_identity(&mut self) {
        if !self.persisted {
            if let Some(note) = &mut self.note {
                note.id = None;
            }
            self.state.id = "draft".to_owned();
            self.state.window_type = "draft".to_owned();
            self.state.note_id = None;
        }
    }

    pub(super) fn retry_window_state(&mut self, now: Instant) {
        self.state_dirty.get_or_insert(now);
        self.state_save_timer.failed(now);
    }

    pub(super) fn save_snapshot(&self, document: Document) -> Result<Note> {
        let note = self
            .note
            .as_ref()
            .context("editable note has no snapshot")?;
        Ok(Note {
            id: note.id.clone().context("draft has no persisted note id")?,
            document,
            theme_id: note.theme_id.clone(),
            created_at: note.created_at,
            updated_at: note.updated_at,
            deleted_at: note.deleted_at,
        })
    }
}
