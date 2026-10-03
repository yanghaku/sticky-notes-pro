//! Note list models and list-window interactions.

use super::*;

impl App {
    pub(super) fn open_list(&mut self, shared: &Rc<RefCell<Self>>) -> Result<()> {
        if let Some(ui) = &self.list {
            platform::activate(ui.window());
            return Ok(());
        }
        let ui = NotesListWindow::new()?;
        self.configure_appearance(&ui);
        ui.set_cards(ModelRc::from(Rc::clone(&self.list_cards)));
        self.restore_list = self.repository.window_state("notes_list")?;
        self.list = Some(ui.clone_strong());
        let weak = Rc::downgrade(shared);
        ui.on_new_note(move || {
            if let Some(shared) = weak.upgrade()
                && let Err(error) = Self::new_note(&shared)
            {
                shared.borrow().report(&format!("{error:#}"));
            }
        });
        let weak = Rc::downgrade(shared);
        ui.on_open_note(move |id| {
            if let Some(shared) = weak.upgrade() {
                let stored = shared.borrow().repository.note(id.as_str());
                match stored {
                    Ok(Some(note)) => {
                        if let Err(error) = Self::open_note(&shared, note) {
                            shared.borrow().report(&format!("{error:#}"));
                        }
                    }
                    Ok(None) => {}
                    Err(error) => shared.borrow().report(&format!("{error:#}")),
                }
            }
        });
        let weak = Rc::downgrade(shared);
        ui.on_settings(move || dispatch(&weak, |app, shared| app.open_settings(shared)));
        let weak = Rc::downgrade(shared);
        ui.on_change_view(move |deleted| {
            dispatch(&weak, |app, _| {
                if let Some(ui) = &app.list {
                    ui.set_deleted_view(deleted);
                }
                app.list_dirty = true;
                app.list_reload = true;
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        ui.on_delete_note(move |id| {
            dispatch(&weak, |app, _| {
                if app.pending_notes.contains(id.as_str()) {
                    return Ok(());
                }
                if let Some(key) = app.window_for_note(id.as_str()).map(str::to_owned) {
                    app.request_close(&key, SaveAction::Delete)?;
                } else {
                    app.worker
                        .submit(Job::Delete {
                            id: id.to_string(),
                            state: None,
                        })
                        .map_err(|e| anyhow::anyhow!("cannot queue deletion: {e}"))?;
                    app.pending_notes.insert(id.to_string());
                }
                app.list_dirty = true;
                app.list_dirty_rows.insert(id.to_string());
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        ui.on_restore_note(move |id| {
            dispatch(&weak, |app, _| {
                if app.pending_notes.contains(id.as_str()) {
                    return Ok(());
                }
                app.worker
                    .submit(Job::Restore { id: id.to_string() })
                    .map_err(|e| anyhow::anyhow!("cannot queue restore: {e}"))?;
                app.pending_notes.insert(id.to_string());
                app.list_dirty = true;
                app.list_dirty_rows.insert(id.to_string());
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        ui.on_purge_note(move |id| {
            dispatch(&weak, |app, shared| {
                app.confirm_permanent_delete(shared, id.as_str())
            });
        });
        let weak = Rc::downgrade(shared);
        let weak_ui = ui.as_weak();
        ui.window().on_close_requested(move || {
            dispatch(&weak, |app, _| {
                if let Some(ui) = weak_ui.upgrade() {
                    if app.closing_list {
                        return Ok(());
                    }
                    let fallback = WindowState::for_aux("notes_list", 620.0, 520.0);
                    let previous = app
                        .list_state
                        .as_ref()
                        .map_or(&fallback, |tracked| &tracked.state);
                    let state = platform::capture(ui.window(), previous);
                    app.worker
                        .submit(Job::CloseWindow { state })
                        .map_err(|e| anyhow::anyhow!("cannot queue window close: {e}"))?;
                    app.closing_list = true;
                }
                Ok(())
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
        self.list_dirty = true;
        self.list_reload = true;
        if let Err(error) = self
            .refresh_list()
            .context("cannot initialize the note list")
            .and_then(|()| ui.show().context("cannot show the note list window"))
        {
            // Both loading and showing can fail after publishing this handle.
            // A hidden handle would prevent reopening the list and exiting.
            self.list = None;
            self.restore_list = None;
            return Err(error);
        }
        self.track_list_window();
        Ok(())
    }

    pub(super) fn refresh_list(&mut self) -> Result<()> {
        let Some(ui) = &self.list else {
            self.list_dirty = false;
            self.list_dirty_rows.clear();
            return Ok(());
        };
        let deleted = ui.get_deleted_view();
        if self.list_reload {
            // Keep only bounded card data. Documents decoded for this structural reload
            // are released immediately; regular edits never query or decode the database.
            let cards = self.repository.map_notes(deleted, |stored| {
                let preview = stored.document.as_ref().map_or_else(
                    |_| preview_plain_text(&stored.plain_text),
                    |document| document.preview_text(4, 240),
                );
                let card = NoteCard {
                    busy: self.pending_notes.contains(&stored.id),
                    id: stored.id.into(),
                    preview: preview_label(preview),
                    created_at: display_timestamp(stored.created_at).into(),
                    updated_at: display_timestamp(stored.updated_at).into(),
                    theme_id: stored.theme_id.into(),
                    status: if deleted {
                        format!(
                            "删除于 {}",
                            display_timestamp(stored.deleted_at.unwrap_or_default())
                        )
                        .into()
                    } else {
                        "".into()
                    },
                };
                self.overlay_card(card, deleted)
            })?;
            let same_rows = cards.len() == self.list_cards.row_count()
                && cards.iter().enumerate().all(|(row, card)| {
                    self.list_cards
                        .row_data(row)
                        .is_some_and(|old| old.id == card.id)
                });
            self.list_rows.clear();
            self.list_rows.extend(
                cards
                    .iter()
                    .enumerate()
                    .map(|(row, card)| (card.id.to_string(), row)),
            );
            if same_rows {
                for (row, card) in cards.into_iter().enumerate() {
                    if self.list_cards.row_data(row).is_some_and(|old| old != card) {
                        self.list_cards.set_row_data(row, card);
                    }
                }
            } else {
                self.list_cards.set_vec(cards);
            }
            self.list_reload = false;
        } else {
            for id in &self.list_dirty_rows {
                let note_id = self
                    .notes
                    .get(id)
                    .and_then(|panel| panel.state.note_id.as_ref())
                    .unwrap_or(id);
                if let Some(&row) = self.list_rows.get(note_id)
                    && let Some(old) = self.list_cards.row_data(row)
                {
                    let card = self.overlay_card(old.clone(), deleted);
                    if card != old {
                        self.list_cards.set_row_data(row, card);
                    }
                }
            }
        }
        self.list_dirty = false;
        self.list_dirty_rows.clear();
        self.list_next_refresh = Instant::now() + Duration::from_millis(120);
        Ok(())
    }

    pub(super) fn overlay_card(&self, mut card: NoteCard, deleted: bool) -> NoteCard {
        card.busy = self.pending_notes.contains(card.id.as_str());
        if !deleted
            && let Some(panel) = self
                .window_for_note(card.id.as_str())
                .and_then(|key| self.notes.get(key))
                .filter(|panel| panel.persisted)
        {
            card.preview = preview_label(panel.editor.as_ref().map_or_else(
                || preview_plain_text(&panel.fallback),
                |editor| editor.document().preview_text(4, 240),
            ));
            if let Some(note) = &panel.note {
                card.theme_id = note.theme_id.as_str().into();
                card.created_at = display_timestamp(note.created_at).into();
                card.updated_at = display_timestamp(note.updated_at).into();
            }
            card.status = panel.ui.get_status();
        }
        card
    }
}

fn preview_label(preview: String) -> slint::SharedString {
    if preview.trim().is_empty() {
        "空便签".into()
    } else {
        preview.into()
    }
}

fn preview_plain_text(text: &str) -> String {
    let mut preview = String::with_capacity(240);
    let mut lines = 1;
    let mut chars = text.chars().peekable();
    for _ in 0..240 {
        let Some(mut ch) = chars.next() else { break };
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            ch = '\n';
        }
        if ch == '\n' {
            if lines == 4 {
                break;
            }
            lines += 1;
        }
        preview.push(ch);
    }
    preview
}

fn display_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp).map_or_else(
        || "未知时间".to_owned(),
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}
