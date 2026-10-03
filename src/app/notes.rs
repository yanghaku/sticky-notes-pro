//! Note window creation, native callbacks and close requests.

use super::*;

impl App {
    pub(super) fn new_note(shared: &Rc<RefCell<Self>>) -> Result<()> {
        let stored = {
            let app = shared.borrow();
            let theme = app.settings.note.default_theme.clone();
            let now = now_millis();
            StoredNote {
                id: uuid::Uuid::new_v4().to_string(),
                document: Ok(Document::default()),
                theme_id: theme,
                plain_text: String::new(),
                created_at: now,
                updated_at: now,
                deleted_at: None,
            }
        };
        Self::open_note(shared, stored)
    }

    pub(super) fn open_note(shared: &Rc<RefCell<Self>>, stored: StoredNote) -> Result<()> {
        let id = stored.id.clone();
        let mut app = shared.borrow_mut();
        if stored.deleted_at.is_some() {
            return Ok(());
        }
        if app.pending_notes.contains(&id) {
            return Ok(());
        }
        if let Some(panel) = app.notes.get(&id) {
            platform::activate(panel.ui.window());
            return Ok(());
        }
        let ui = NoteWindow::new()?;
        let persisted = app.repository.note_exists(&id)?;
        ui.set_status(if persisted {
            "已保存".into()
        } else {
            "".into()
        });
        ui.set_editor_font_family(app.settings.editor.font_family.as_str().into());
        ui.set_editor_font_size(app.settings.editor.font_size as f32 * app.settings.scale_factor());
        let state = app
            .repository
            .window_state(&format!("note:{id}"))
            // Geometry is cosmetic: a damaged row must not stop the note opening.
            .unwrap_or_else(|error| {
                eprintln!("cannot read window state for note {id}: {error:#}");
                None
            })
            .unwrap_or_else(|| {
                let inherited = app
                    .last_active
                    .as_ref()
                    .and_then(|id| app.notes.get(id))
                    .map(|panel| platform::capture(panel.ui.window(), &panel.state))
                    .or_else(|| match app.repository.recent_note_window() {
                        Ok(state) => state,
                        Err(error) => {
                            eprintln!("cannot read recent note window: {error:#}");
                            None
                        }
                    });
                let mut state = WindowState::for_note(
                    &id,
                    inherited
                        .as_ref()
                        .map_or(app.settings.note.default_width, |s| s.normal_width),
                    inherited
                        .as_ref()
                        .map_or(app.settings.note.default_height, |s| s.normal_height),
                );
                if let Some(old) = inherited {
                    state.normal_x = old.normal_x.map(|x| x + 20.0);
                    state.normal_y = old.normal_y.map(|y| y + 20.0);
                    state.monitor_id = old.monitor_id;
                    state.scale_factor = old.scale_factor;
                }
                state
            });
        ui.window().set_size(slint::LogicalSize::new(
            state.normal_width as f32,
            state.normal_height as f32,
        ));
        ui.set_theme_id(stored.theme_id.as_str().into());
        app.configure_appearance(&ui);
        ui.set_pinned(state.always_on_top);
        let (note, editor) = match stored.document {
            Ok(document) => {
                let editor = Editor::new(document);
                (
                    Some(NoteMetadata {
                        id: id.clone(),
                        theme_id: stored.theme_id,
                        created_at: stored.created_at,
                        updated_at: stored.updated_at,
                        deleted_at: None,
                    }),
                    Some(editor),
                )
            }
            Err(error) => {
                ui.set_readonly(true);
                ui.set_readonly_text(stored.plain_text.as_str().into());
                ui.set_error(format!("无法编辑这张便签，原始数据已保留：{error}").into());
                ui.set_status("只读".into());
                (None, None)
            }
        };
        let was_empty = editor.as_ref().is_none_or(Editor::is_empty);
        let fallback = if editor.is_some() {
            // Editable notes already own their text in Editor and its layout.
            drop(stored.plain_text);
            String::new()
        } else {
            stored.plain_text
        };
        let mut layout = TextLayout::new(Rc::clone(&app.text_resources));
        layout.set_font_family(&app.settings.editor.font_family);
        layout.set_dark(ui.global::<AppAppearance>().get_dark());
        app.notes.insert(
            id.clone(),
            Panel {
                ui: ui.clone_strong(),
                note,
                persisted,
                last_nonempty: None,
                was_empty,
                confirming_empty: false,
                empty_confirmed: false,
                editor,
                fallback,
                state,
                layout,
                preedit: String::new(),
                ime: ImeState::default(),
                modifiers: ModifiersState::empty(),
                scroll: 0.0,
                revision: 0,
                saved_revision: 0,
                in_flight: false,
                destructive_queued: false,
                pending_action: None,
                waiting_ime: false,
                dirty_since: None,
                ime_wait_started: None,
                last_edit: Instant::now(),
                last_submit: Instant::now(),
                render_dirty: true,
                follow_caret: true,
                state_dirty: None,
                state_in_flight: false,
                state_failure_status: None,
                state_save_timer: WindowSaveTimer::default(),
                discarding: false,
                view: (0.0, 0.0, 0.0),
                restored: false,
            },
        );
        app.last_active = Some(id.clone());
        app.list_dirty = true;
        app.list_reload |= persisted;
        drop(app);
        Self::wire_note(shared, &id, &ui);
        ui.invoke_focus_editor();
        if let Err(error) = ui.show() {
            // The native window never appeared: release the panel, or it would
            // still accept saves, hold the note list open and block a clean exit.
            if let Ok(mut app) = shared.try_borrow_mut() {
                if app.last_active.as_deref() == Some(id.as_str()) {
                    app.last_active = None;
                }
                app.notes.remove(&id);
                app.list_dirty = true;
                app.list_reload = true;
            }
            return Err(error).context("cannot show the note window");
        }
        Ok(())
    }

    pub(super) fn wire_note(shared: &Rc<RefCell<Self>>, id: &str, ui: &NoteWindow) {
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_close_note(move || {
            dispatch(&weak, |app, _| {
                app.request_close(&note_id, SaveAction::Close)
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.window().on_close_requested(move || {
            dispatch(&weak, |app, _| {
                app.request_close(&note_id, SaveAction::Close)
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
        let weak = Rc::downgrade(shared);
        ui.on_open_list(move || dispatch(&weak, |app, shared| app.open_list(shared)));
        let weak = Rc::downgrade(shared);
        ui.on_open_settings(move || dispatch(&weak, |app, shared| app.open_settings(shared)));
        let weak_ui = ui.as_weak();
        ui.on_drag_window(move || {
            if let Some(ui) = weak_ui.upgrade()
                && let Err(error) = platform::drag(ui.window())
            {
                eprintln!("cannot drag window: {error}");
            }
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_pin(move || {
            dispatch(&weak, |app, _| {
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    panel.state.always_on_top = !panel.state.always_on_top;
                    panel.ui.set_pinned(panel.state.always_on_top);
                    panel.state_dirty = Some(Instant::now() - Duration::from_secs(1));
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_choose_theme(move |theme| {
            dispatch(&weak, |app, _| {
                if let Some(panel) = app.notes.get_mut(&note_id)
                    && !panel.ui.get_frozen()
                    && let Some(note) = &mut panel.note
                {
                    if note.theme_id == theme.as_str() {
                        return Ok(());
                    }
                    note.theme_id = theme.to_string();
                    panel.ui.set_theme_id(theme.clone());
                    panel.changed();
                    app.list_dirty = true;
                    app.list_dirty_rows.insert(note_id.clone());
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_pointer(move |x, y, drag| {
            dispatch(&weak, |app, _| {
                let app_scale = app.settings.scale_factor();
                let font_size = app.settings.editor.font_size as f32;
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    panel.refresh_layout(font_size);
                }
                if let Some(panel) = app.notes.get_mut(&note_id)
                    && !panel.ui.get_frozen()
                    && panel.preedit.is_empty()
                    && let Some(position) = panel.layout.hit(x / app_scale, y / app_scale)
                    && let Some(editor) = &mut panel.editor
                {
                    let anchor = if drag || panel.modifiers.shift_key() {
                        editor.selection().0
                    } else {
                        position
                    };
                    editor.select(anchor, position);
                    panel.ui.invoke_focus_editor();
                    panel.set_ime_enabled(true);
                    panel.render_dirty = true;
                    panel.follow_caret = true;
                    app.last_active = Some(note_id.clone());
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_wheel(move |delta| {
            dispatch(&weak, |app, _| {
                let app_scale = app.settings.scale_factor();
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    panel.scroll = (panel.scroll - delta / app_scale).max(0.0);
                    panel.render_dirty = true;
                    panel.follow_caret = false;
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_retry(move || {
            dispatch(&weak, |app, _| {
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    if panel.editor.is_none() || panel.discarding {
                        return Ok(());
                    }
                    panel.ui.set_error("".into());
                    panel.dirty_since = Some(Instant::now());
                    panel.last_edit = Instant::now() - Duration::from_secs(1);
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_copy_all(move || {
            dispatch(&weak, |app, _| {
                let panel = app.notes.get(&note_id).context("note window is closed")?;
                let text = panel
                    .editor
                    .as_ref()
                    .map_or_else(|| panel.fallback.clone(), |e| e.document().display_text());
                app.clipboard
                    .set_text(panel.ui.window(), &text)
                    .context("cannot copy note")
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_discard(move || dispatch(&weak, |app, shared| app.confirm_discard(shared, &note_id)));
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.window().on_winit_window_event(move |window, event| {
            let Some(shared) = weak.upgrade() else {
                return EventResult::Propagate;
            };
            let Ok(mut app) = shared.try_borrow_mut() else {
                return EventResult::Propagate;
            };
            if matches!(event, WindowEvent::RedrawRequested) {
                let font_size = app.settings.editor.font_size as f32;
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    panel.paint(font_size);
                }
                // Slint's native filter runs before its draw(), so the updated
                // image belongs to this frame. Do not enqueue another redraw.
                return EventResult::Propagate;
            }
            let result = app.input(&shared, &note_id, window, event);
            app.schedule_render();
            match result {
                Ok(consumed) => {
                    if consumed {
                        EventResult::PreventDefault
                    } else {
                        EventResult::Propagate
                    }
                }
                Err(error) => {
                    app.report(&format!("{error:#}"));
                    EventResult::PreventDefault
                }
            }
        });
    }

    pub(super) fn request_close(&mut self, id: &str, action: SaveAction) -> Result<()> {
        if let Some(panel) = self.notes.get_mut(id) {
            if panel.pending_action.is_some() || panel.discarding || panel.confirming_empty {
                return Ok(());
            }
            panel.pending_action = Some(action);
            if action == SaveAction::Delete {
                self.pending_notes.insert(id.to_owned());
                self.list_dirty_rows.insert(id.to_owned());
                self.list_dirty = true;
            }
            panel.ui.set_frozen(true);
            panel.ui.invoke_dismiss_popups();
            if !panel.preedit.is_empty() {
                panel.waiting_ime = true;
                panel.ime_wait_started = Some(Instant::now());
            }
            panel.set_ime_enabled(false);
        }
        Ok(())
    }
}
