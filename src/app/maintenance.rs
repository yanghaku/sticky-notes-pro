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
        let mut empty_requests = Vec::new();
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
        for panel in self.notes.values_mut() {
            if !panel.restored && panel.ui.window().has_winit_window() {
                panel.update_window_limits(panel.state.monitor_id.as_deref());
                platform::restore(panel.ui.window(), &panel.state);
                let capabilities = platform::capabilities(panel.ui.window());
                panel.ui.set_pin_supported(capabilities.pin);
                if capabilities.is_wayland {
                    panel.ui.set_status("Wayland：位置与置顶由桌面管理".into());
                    if let Some(id) = &panel.state.note_id {
                        self.list_dirty_rows.insert(id.clone());
                    }
                    self.list_dirty = true;
                }
                panel.restored = true;
                panel.set_ime_enabled(panel.editor.is_some());
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
                || panel.discarding
                || panel.confirming_empty;
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
                && !panel.confirming_empty
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
            if empty && panel.persisted && !panel.empty_confirmed {
                if let Some(id) = &panel.state.note_id {
                    empty_requests.push(id.clone());
                }
                continue;
            }
            if empty && action == SaveAction::Save {
                panel.dirty_since = None;
                panel.ui.set_status("".into());
                if let Some(id) = &panel.state.note_id {
                    self.list_dirty_rows.insert(id.clone());
                }
                self.list_dirty = true;
                continue;
            }
            let job = if empty
                && panel.persisted
                && panel.empty_confirmed
                && panel.last_nonempty.is_some()
            {
                let note = panel.save_snapshot(
                    panel
                        .last_nonempty
                        .clone()
                        .context("cleared note has no prior content")?,
                )?;
                Job::Save {
                    note,
                    state: Some(panel.state.clone()),
                    revision: panel.revision,
                    action: SaveAction::Delete,
                    create: false,
                }
            } else if empty {
                Job::CloseEmpty {
                    id: panel
                        .state
                        .note_id
                        .clone()
                        .context("note window has no id")?,
                    state: panel.state.clone(),
                }
            } else if panel.note.is_some() {
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
                    panel.destructive_queued = empty || action == SaveAction::Delete;
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
        if let Some(id) = empty_requests.first() {
            self.confirm_empty_delete(shared, id)?;
        }
        if self.list_dirty && (self.list_reload || now >= self.list_next_refresh) {
            self.refresh_list()?;
        }
        Ok(())
    }
}

impl Panel {
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
            id: note.id.clone(),
            document,
            theme_id: note.theme_id.clone(),
            created_at: note.created_at,
            updated_at: note.updated_at,
            deleted_at: note.deleted_at,
        })
    }
}
