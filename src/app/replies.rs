//! Persistence worker completions and failure recovery.

use super::*;

impl App {
    pub(super) fn process_replies(&mut self) -> Result<()> {
        while let Some(reply) = self.worker.poll() {
            let window_key = self.reply_window(&reply.id);
            if matches!(
                reply.kind,
                ReplyKind::Delete | ReplyKind::Restore | ReplyKind::DraftClosed | ReplyKind::Purge
            ) {
                self.pending_notes.remove(&reply.id);
            }
            if let Some(warning) = &reply.warning {
                eprintln!("window state was not saved: {warning}");
                let message = format!("内容已保存，但窗口状态未保存：{warning}");
                // The warning belongs to one window: only that surface changes,
                // so another note's status or a real error is never overwritten.
                let note = reply.id.strip_prefix("note:").unwrap_or(&reply.id);
                if let Some(panel) = window_key.as_ref().and_then(|key| self.notes.get_mut(key)) {
                    if reply.kind == ReplyKind::Save {
                        panel.retry_window_state(Instant::now());
                    }
                    panel.ui.set_status(message.as_str().into());
                    self.list_dirty_rows.insert(note.to_owned());
                    self.list_dirty = true;
                } else if reply.id == "notes_list"
                    && let Some(ui) = &self.list
                    && ui.get_error().is_empty()
                {
                    ui.set_error(message.as_str().into());
                } else if reply.id == "settings"
                    && let Some(ui) = &self.settings_ui
                    && ui.get_status().is_empty()
                {
                    ui.set_status(message.as_str().into());
                }
            }
            match reply.result {
                Ok(()) => match reply.kind {
                    ReplyKind::Close => {
                        if let Some(panel) =
                            window_key.as_ref().and_then(|key| self.notes.get_mut(key))
                            && panel.revision != reply.revision
                        {
                            // A late IME commit must be flushed before releasing the window.
                            panel.in_flight = false;
                            self.list_reload |= !panel.persisted;
                            panel.persisted = true;
                            panel.saved_revision = reply.revision;
                            panel.pending_action = Some(SaveAction::Close);
                        } else {
                            self.remove_note(window_key.as_deref().unwrap_or(&reply.id))?;
                        }
                    }
                    ReplyKind::Delete => {
                        self.remove_note(window_key.as_deref().unwrap_or(&reply.id))?;
                        self.list_dirty = true;
                        self.list_reload = true;
                    }
                    ReplyKind::DraftClosed => {
                        self.remove_note(window_key.as_deref().unwrap_or(&reply.id))?;
                        self.list_dirty = true;
                        self.list_reload = true;
                    }
                    ReplyKind::Restore => {
                        self.list_dirty = true;
                        self.list_reload = true;
                    }
                    ReplyKind::Purge => {
                        if self
                            .permanent_dialog
                            .as_ref()
                            .is_some_and(|(id, _)| id == &reply.id)
                            && let Some((_, ui)) = self.permanent_dialog.take()
                        {
                            ui.hide()?;
                        }
                        self.list_dirty = true;
                        self.list_reload = true;
                        self.check_exit()?;
                    }
                    ReplyKind::Save => {
                        if let Some(panel) =
                            window_key.as_ref().and_then(|key| self.notes.get_mut(key))
                        {
                            panel.in_flight = false;
                            panel.destructive_queued = false;
                            self.list_reload |= !panel.persisted;
                            panel.persisted = true;
                            panel.saved_revision = reply.revision;
                            if panel.saved_revision == panel.revision {
                                panel.dirty_since = None;
                                panel.ui.set_status("已保存".into());
                            } else {
                                panel.ui.set_status("未保存".into());
                            }
                        }
                    }
                    ReplyKind::Window => {
                        if reply.id.starts_with("note:") {
                            if window_key
                                .as_ref()
                                .and_then(|key| self.notes.get(key))
                                .is_some_and(|p| {
                                    p.discarding || (p.pending_action.is_some() && p.note.is_none())
                                })
                            {
                                self.remove_note(
                                    window_key
                                        .as_deref()
                                        .context("closing note has no window")?,
                                )?;
                            }
                        } else if reply.id == "notes_list" {
                            self.list_state = None;
                            if let Some(ui) = self.list.take() {
                                ui.hide()?;
                            }
                            self.closing_list = false;
                            self.list_cards.set_vec(Vec::new());
                            self.list_rows = HashMap::new();
                            self.list_dirty_rows = HashSet::new();
                            self.list_reload = true;
                            self.check_exit()?;
                        } else if reply.id == "settings" {
                            self.settings_state = None;
                            self.color_draft = None;
                            if let Some(ui) = self.settings_ui.take() {
                                ui.hide()?;
                            }
                            self.closing_settings = false;
                            self.check_exit()?;
                        }
                    }
                    ReplyKind::WindowSaved => {
                        if let Some(panel) =
                            window_key.as_ref().and_then(|key| self.notes.get_mut(key))
                        {
                            panel.state_in_flight = false;
                            if let Some(message) = panel.state_failure_status.take()
                                && panel.ui.get_status() == message
                            {
                                panel.ui.set_status(
                                    if panel.note.is_none() {
                                        "只读"
                                    } else if panel.saved_revision == panel.revision {
                                        "已保存"
                                    } else {
                                        "未保存"
                                    }
                                    .into(),
                                );
                                self.list_dirty_rows.insert(
                                    reply
                                        .id
                                        .strip_prefix("note:")
                                        .unwrap_or(&reply.id)
                                        .to_owned(),
                                );
                                self.list_dirty = true;
                            }
                        } else if reply.id == "notes_list" {
                            let previous_message = self
                                .list_state
                                .as_mut()
                                .and_then(|tracked| tracked.completed(None));
                            if let Some(message) = previous_message
                                && let Some(ui) = &self.list
                                && ui.get_error() == message
                            {
                                ui.set_error("".into());
                            }
                        } else if reply.id == "settings" {
                            let previous_message = self
                                .settings_state
                                .as_mut()
                                .and_then(|tracked| tracked.completed(None));
                            if let Some(message) = previous_message
                                && let Some(ui) = &self.settings_ui
                                && ui.get_status() == message
                            {
                                ui.set_status("".into());
                            }
                        }
                    }
                },
                Err(error) => {
                    if reply.kind == ReplyKind::WindowSaved {
                        let tracked = match reply.id.as_str() {
                            "notes_list" => self.list_state.as_mut(),
                            "settings" => self.settings_state.as_mut(),
                            _ => None,
                        };
                        if let Some(tracked) = tracked {
                            let message: slint::SharedString =
                                format!("窗口状态保存失败：{error}").into();
                            tracked.completed(Some(message.clone()));
                            if reply.id == "notes_list" {
                                if let Some(ui) = &self.list {
                                    ui.set_error(message);
                                }
                            } else if let Some(ui) = &self.settings_ui {
                                ui.set_status(message);
                            }
                            eprintln!("window state save failed: {error}");
                            continue;
                        }
                    }
                    if reply.kind == ReplyKind::Purge {
                        if let Some((id, ui)) = &self.permanent_dialog
                            && id == &reply.id
                        {
                            ui.set_busy(false);
                            ui.set_error(format!("彻底删除失败：{error}").into());
                        } else {
                            self.report(&error);
                        }
                        self.list_dirty_rows.insert(reply.id.clone());
                        self.list_dirty = true;
                        continue;
                    }
                    let id = reply.id.strip_prefix("note:").unwrap_or(&reply.id);
                    if let Some(panel) = window_key.as_ref().and_then(|key| self.notes.get_mut(key))
                    {
                        if reply.kind == ReplyKind::WindowSaved {
                            panel.state_in_flight = false;
                            panel.retry_window_state(Instant::now());
                            let message: slint::SharedString = "窗口状态保存失败".into();
                            panel.ui.set_status(message.clone());
                            panel.state_failure_status = Some(message);
                            eprintln!("window state save failed: {error}");
                            self.list_dirty_rows.insert(id.to_owned());
                            self.list_dirty = true;
                            continue;
                        }
                        panel.in_flight = false;
                        panel.retry_window_state(Instant::now());
                        panel.destructive_queued = false;
                        panel.pending_action = None;
                        panel.discarding = false;
                        self.pending_notes.remove(id);
                        panel.waiting_ime = false;
                        panel.ime_wait_started = None;
                        panel.ui.set_frozen(false);
                        panel.ui.set_error(error.as_str().into());
                        panel.ui.set_status("保存失败".into());
                        panel.dirty_since = None;
                        if !panel.persisted && panel.editor.as_ref().is_some_and(Editor::is_empty) {
                            panel.clear_draft_identity();
                        }
                        panel.set_ime_enabled(panel.editor.is_some());
                    } else {
                        self.report(&error);
                    }
                }
            }
            if !matches!(reply.kind, ReplyKind::WindowSaved | ReplyKind::Window) {
                self.list_dirty = true;
                self.list_dirty_rows.insert(
                    reply
                        .id
                        .strip_prefix("note:")
                        .unwrap_or(&reply.id)
                        .to_owned(),
                );
            }
        }
        Ok(())
    }
}
