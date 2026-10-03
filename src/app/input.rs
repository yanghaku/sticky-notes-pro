//! Native keyboard, clipboard and IME routing.

use super::*;

impl App {
    pub(super) fn input(
        &mut self,
        shared: &Rc<RefCell<Self>>,
        id: &str,
        window: &slint::Window,
        event: &WindowEvent,
    ) -> Result<bool> {
        if matches!(event, WindowEvent::Moved(_)) {
            // Only the saved rectangle changes: a drag must not re-shape and
            // re-raster the whole document sixty times a second.
            if let Some(panel) = self.notes.get_mut(id) {
                if panel.restored {
                    panel.update_window_limits(None);
                }
                panel.state_dirty = Some(Instant::now());
            }
        } else if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) && let Some(panel) = self.notes.get_mut(id)
        {
            if panel.restored {
                panel.update_window_limits(None);
            }
            panel.state_dirty = Some(Instant::now());
            panel.render_dirty = true;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            if let Some(panel) = self.notes.get_mut(id) {
                panel.modifiers = modifiers.state();
            }
            return Ok(false);
        }
        if let WindowEvent::Focused(focused) = event {
            if *focused {
                self.last_active = Some(id.to_owned());
                if let Some(panel) = self.notes.get_mut(id) {
                    panel.state.last_used_at = now_millis();
                    panel.state_dirty = Some(Instant::now());
                    panel.set_ime_enabled(panel.editor.is_some() && !panel.ui.get_frozen());
                }
            } else if let Some(panel) = self.notes.get_mut(id) {
                if !panel.preedit.is_empty() {
                    panel.waiting_ime = true;
                    panel.ime_wait_started = Some(Instant::now());
                }
                panel.set_ime_enabled(false);
            }
            return Ok(false);
        }
        if let WindowEvent::Ime(ime) = event {
            if let Some(panel) = self.notes.get_mut(id) {
                match ime {
                    Ime::Preedit(text, cursor) => {
                        let cursor_changed = panel.layout.set_preedit_cursor(*cursor);
                        if panel.preedit != *text || cursor_changed {
                            panel.preedit.clone_from(text);
                            panel.render_dirty = true;
                            panel.follow_caret = true;
                        }
                    }
                    Ime::Commit(text) => {
                        let had_preedit = !panel.preedit.is_empty();
                        panel.preedit.clear();
                        let cursor_changed = panel.layout.set_preedit_cursor(None);
                        if had_preedit || cursor_changed {
                            panel.render_dirty = true;
                            panel.follow_caret = true;
                        }
                        if panel.destructive_queued || panel.discarding || panel.confirming_empty {
                            // Destructive snapshots are immutable once queued. Composition
                            // was finished/cancelled before freezing, so late native events
                            // cannot mutate a deleted or discarded document.
                            panel.waiting_ime = false;
                            panel.ime_wait_started = None;
                            return Ok(true);
                        }
                        // An empty native commit finishes composition; it must
                        // not run insertion's selected-text replacement path.
                        if !text.is_empty()
                            && let Some(editor) = &mut panel.editor
                            && editor.commit_text(text)
                        {
                            panel.changed();
                            self.list_dirty = true;
                            self.list_dirty_rows.insert(id.to_owned());
                        }
                        panel.waiting_ime = false;
                        panel.ime_wait_started = None;
                    }
                    Ime::Disabled => {
                        let had_preedit = !panel.preedit.is_empty();
                        panel.preedit.clear();
                        let cursor_changed = panel.layout.set_preedit_cursor(None);
                        panel.waiting_ime = false;
                        panel.ime_wait_started = None;
                        panel.ime.disabled();
                        if had_preedit || cursor_changed {
                            panel.render_dirty = true;
                            panel.follow_caret = true;
                        }
                    }
                    Ime::Enabled => panel.ime.enabled(window),
                }
            }
            return Ok(true);
        }
        let WindowEvent::KeyboardInput {
            event,
            is_synthetic,
            ..
        } = event
        else {
            return Ok(false);
        };
        if *is_synthetic {
            return Ok(true);
        }
        let Some(panel) = self.notes.get(id) else {
            return Ok(false);
        };
        if panel.ui.get_menu_open() {
            return Ok(false);
        }
        if event.state != ElementState::Pressed {
            return Ok(true);
        }
        if let Some(action) = shortcuts::note_action(&event.logical_key, panel.modifiers) {
            if event.repeat {
                return Ok(true);
            }
            if panel.ui.get_frozen() || !panel.preedit.is_empty() || panel.waiting_ime {
                return Ok(true);
            }
            // Do not create a window while the native-event callback holds App's borrow.
            match action {
                NoteShortcut::NewNote => {
                    let weak = Rc::downgrade(shared);
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(shared) = weak.upgrade()
                            && let Err(error) = Self::new_note(&shared)
                        {
                            shared.borrow().report(&format!("{error:#}"));
                        }
                    });
                }
                NoteShortcut::NotesList => self.open_list(shared)?,
                NoteShortcut::Settings => self.open_settings(shared)?,
                NoteShortcut::Close => self.request_close(id, SaveAction::Close)?,
                NoteShortcut::Save => {
                    if let Some(panel) = self.notes.get_mut(id)
                        && panel.editor.is_some()
                        && !panel.in_flight
                    {
                        panel.ui.set_error("".into());
                        panel.dirty_since.get_or_insert(Instant::now());
                        panel.last_edit = Instant::now() - Duration::from_secs(1);
                    }
                }
                NoteShortcut::TogglePin => {
                    if let Some(panel) = self.notes.get_mut(id)
                        && panel.ui.get_pin_supported()
                    {
                        panel.state.always_on_top = !panel.state.always_on_top;
                        panel.ui.set_pinned(panel.state.always_on_top);
                        panel.state_dirty = Some(Instant::now() - Duration::from_secs(1));
                    }
                }
            }
            return Ok(true);
        }
        if !panel.ui.get_editor_focused() || !panel.ui.get_error().is_empty() {
            return Ok(false);
        }
        let Some(panel) = self.notes.get(id) else {
            return Ok(false);
        };
        if panel.ui.get_frozen() || panel.editor.is_none() {
            return Ok(true);
        }
        if !panel.preedit.is_empty() || panel.waiting_ime {
            return Ok(true);
        }
        let command = shortcuts::primary_pressed(panel.modifiers);
        let shift = panel.modifiers.shift_key();
        if command && let Key::Character(key) = &event.logical_key {
            match key.to_lowercase().as_str() {
                "c" | "x" => {
                    let text = panel
                        .editor
                        .as_ref()
                        .map(Editor::clipboard_text)
                        .unwrap_or_default();
                    if !text.is_empty() {
                        let cutting = key.eq_ignore_ascii_case("x");
                        self.clipboard
                            .set_text(window, &text)
                            .context("cannot copy selection")?;
                        if cutting && platform::capabilities(window).is_wayland {
                            self.check_wayland_cut(shared, id, text);
                            return Ok(true);
                        }
                        if cutting
                            && let Some(panel) = self.notes.get_mut(id)
                            && let Some(editor) = &mut panel.editor
                            && editor.backspace()
                        {
                            panel.changed();
                            self.list_dirty = true;
                            self.list_dirty_rows.insert(id.to_owned());
                        }
                    }
                    return Ok(true);
                }
                "v" => {
                    let text = self.clipboard.text(window)?;
                    if let Some(panel) = self.notes.get_mut(id) {
                        if let Some(editor) = &mut panel.editor
                            && editor.insert_text(&text, false)
                        {
                            panel.changed();
                            self.list_dirty = true;
                            self.list_dirty_rows.insert(id.to_owned());
                        }
                        // A paste that matches the selection still moves the caret.
                        panel.render_dirty = true;
                        panel.follow_caret = true;
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }
        let Some(panel) = self.notes.get_mut(id) else {
            return Ok(false);
        };
        if matches!(
            event.logical_key,
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
        ) {
            panel.refresh_layout(self.settings.editor.font_size as f32);
        }
        let Some(editor) = &mut panel.editor else {
            return Ok(false);
        };
        let old_revision = editor.revision();
        match &event.logical_key {
            Key::Character(key) if command => match key.to_lowercase().as_str() {
                "a" => editor.select_all(),
                "z" if shift => {
                    editor.redo();
                }
                "z" => {
                    editor.undo();
                }
                "y" => {
                    editor.redo();
                }
                _ => return Ok(false),
            },
            Key::Named(NamedKey::Enter) => {
                editor.enter();
            }
            Key::Named(NamedKey::Backspace) => {
                editor.backspace();
            }
            Key::Named(NamedKey::Delete) => {
                editor.delete_forward();
            }
            Key::Named(NamedKey::ArrowLeft) => editor.move_left(shift),
            Key::Named(NamedKey::ArrowRight) => editor.move_right(shift),
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown) => {
                let delta = if event.logical_key == Key::Named(NamedKey::ArrowUp) {
                    -1
                } else {
                    1
                };
                if let Some(position) = panel.layout.vertical(editor.selection().1, delta) {
                    let anchor = if shift {
                        editor.selection().0
                    } else {
                        position
                    };
                    editor.select(anchor, position);
                } else {
                    editor.move_vertical(delta as isize, shift);
                }
            }
            Key::Named(NamedKey::Home) if command => editor.document_home(shift),
            Key::Named(NamedKey::End) if command => editor.document_end(shift),
            Key::Named(NamedKey::Home) => editor.home(shift),
            Key::Named(NamedKey::End) => editor.end(shift),
            Key::Named(NamedKey::Escape) => return Ok(false),
            Key::Named(NamedKey::Tab) => return Ok(false),
            _ if !command && !panel.modifiers.alt_key() => {
                if let Some(text) = &event.text
                    && !text.chars().any(char::is_control)
                {
                    editor.insert_text(text, true);
                }
            }
            _ => return Ok(false),
        }
        if editor.revision() != old_revision {
            panel.changed();
            self.list_dirty = true;
            self.list_dirty_rows.insert(id.to_owned());
        }
        panel.render_dirty = true;
        panel.follow_caret = true;
        Ok(true)
    }
}
