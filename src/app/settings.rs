//! Settings-window drafts and committed preferences.

use super::*;

impl App {
    pub(super) fn reset_settings_values(&mut self) {
        let Some(ui) = &self.settings_ui else { return };
        ui.set_note_width(self.settings.note.default_width.round() as i32);
        ui.set_note_height(self.settings.note.default_height.round() as i32);
        ui.set_font_size_value(self.settings.editor.font_size.round() as i32);
        ui.set_window_font_size_value(self.settings.window_font.font_size.round() as i32);
        ui.set_scale_percent_value(i32::from(self.settings.scale_percent));
        ui.set_window_font_family_index(
            self.font_families
                .iter()
                .position(|family| *family == self.settings.window_font.font_family)
                .unwrap_or(0) as i32,
        );
        ui.set_font_family_index(
            self.font_families
                .iter()
                .position(|family| *family == self.settings.editor.font_family)
                .unwrap_or(0) as i32,
        );
        ui.set_theme_index(
            THEMES
                .iter()
                .position(|theme| *theme == self.settings.note.default_theme)
                .unwrap_or(0) as i32,
        );
        ui.set_appearance_index(self.settings.appearance_mode.index());
        self.reset_color_values();
    }

    pub(super) fn close_settings(&mut self) -> Result<()> {
        if self.closing_settings {
            return Ok(());
        }
        let Some(ui) = &self.settings_ui else {
            return Ok(());
        };
        let fallback = WindowState::for_aux("settings", 420.0, 530.0);
        let previous = self
            .settings_state
            .as_ref()
            .map_or(&fallback, |tracked| &tracked.state);
        let state = platform::capture(ui.window(), previous);
        self.worker
            .submit(Job::CloseWindow { state })
            .map_err(|e| anyhow::anyhow!("cannot queue window close: {e}"))?;
        self.closing_settings = true;
        Ok(())
    }

    pub(super) fn open_settings(&mut self, shared: &Rc<RefCell<Self>>) -> Result<()> {
        if let Some(ui) = &self.settings_ui {
            platform::activate(ui.window());
            return Ok(());
        }
        let ui = SettingsWindow::new()?;
        ui.set_shortcuts(ModelRc::new(VecModel::from(shortcuts::descriptions())));
        self.configure_appearance(&ui);
        ui.set_appearance_index(self.settings.appearance_mode.index());
        self.restore_settings = self.repository.window_state("settings")?;
        ui.set_note_width(self.settings.note.default_width.round() as i32);
        ui.set_note_height(self.settings.note.default_height.round() as i32);
        ui.set_font_size_value(self.settings.editor.font_size.round() as i32);
        if !self
            .font_families
            .contains(&self.settings.editor.font_family)
        {
            self.font_families
                .push(self.settings.editor.font_family.clone());
        }
        if !self
            .font_families
            .contains(&self.settings.window_font.font_family)
        {
            self.font_families
                .push(self.settings.window_font.font_family.clone());
        }
        ui.set_font_families(ModelRc::new(VecModel::from(
            self.font_families
                .iter()
                .map(|family| {
                    if family.is_empty() {
                        "系统默认".into()
                    } else {
                        family.as_str().into()
                    }
                })
                .collect::<Vec<slint::SharedString>>(),
        )));
        ui.set_font_family_index(
            self.font_families
                .iter()
                .position(|family| *family == self.settings.editor.font_family)
                .unwrap_or(0) as i32,
        );
        ui.set_theme_index(
            THEMES
                .iter()
                .position(|theme| *theme == self.settings.note.default_theme)
                .unwrap_or(0) as i32,
        );
        ui.set_writable(self.settings_writable);
        if let Some(warning) = &self.settings_warning {
            ui.set_status(warning.as_str().into());
        }
        self.settings_ui = Some(ui.clone_strong());
        self.color_draft = None;
        self.reset_settings_values();
        self.wire_colors(shared, &ui);
        ui.invoke_preferences_saved();
        let weak = Rc::downgrade(shared);
        ui.on_discard_changes(move || {
            dispatch(&weak, |app, _| {
                app.reset_settings_values();
                if let Some(ui) = &app.settings_ui {
                    ui.set_status(app.settings_warning.as_deref().unwrap_or("").into());
                    ui.invoke_preferences_saved();
                }
                Ok(())
            });
        });
        let weak = Rc::downgrade(shared);
        ui.on_cancel_settings(move || dispatch(&weak, |app, _| app.close_settings()));
        let weak = Rc::downgrade(shared);
        ui.on_apply(
            move |width,
                  height,
                  font,
                  theme,
                  family,
                  appearance,
                  window_family,
                  window_size,
                  scale| {
                dispatch(&weak, |app, _| {
                    let mut updated = app.settings.clone();
                    updated.note.colors = match app.color_values_for_save() {
                        Ok(colors) => colors,
                        Err(error) => {
                            if let Some(ui) = &app.settings_ui {
                                ui.set_status(format!("保存失败：{error:#}").into());
                            }
                            return Ok(());
                        }
                    };
                    updated.appearance_mode = AppearanceMode::from_index(appearance)
                        .context("invalid appearance mode")?;
                    updated.note.default_width = f64::from(width);
                    updated.note.default_height = f64::from(height);
                    updated.editor.font_size = f64::from(font);
                    updated.note.default_theme = THEMES
                        .get(usize::try_from(theme).unwrap_or(0))
                        .unwrap_or(&"white")
                        .to_string();
                    updated.editor.font_family = app
                        .font_families
                        .get(usize::try_from(family).unwrap_or(0))
                        .cloned()
                        .unwrap_or_default();
                    updated.window_font.font_size = f64::from(window_size);
                    updated.scale_percent =
                        u16::try_from(scale).context("invalid application scale")?;
                    updated.window_font.font_family = app
                        .font_families
                        .get(usize::try_from(window_family).unwrap_or(0))
                        .cloned()
                        .unwrap_or_default();
                    if let Err(error) = updated.save(&app.paths.settings_file) {
                        if let Some(ui) = &app.settings_ui {
                            ui.set_status(format!("保存失败：{error:#}").into());
                        }
                    } else {
                        let zoom_changed = app.settings.scale_percent != updated.scale_percent;
                        app.settings = updated;
                        app.apply_appearance();
                        for panel in app.notes.values_mut() {
                            panel
                                .layout
                                .set_font_family(&app.settings.editor.font_family);
                            panel.ui.set_editor_font_family(
                                app.settings.editor.font_family.as_str().into(),
                            );
                            panel.ui.set_editor_font_size(
                                app.settings.editor.font_size as f32 * app.settings.scale_factor(),
                            );
                            panel.render_dirty = true;
                            panel.follow_caret |= zoom_changed;
                        }
                        app.reset_settings_values();
                        if let Some(ui) = &app.settings_ui {
                            ui.set_status("设置已保存".into());
                            ui.invoke_preferences_saved();
                        }
                    }
                    Ok(())
                })
            },
        );
        let weak = Rc::downgrade(shared);
        ui.window().on_close_requested(move || {
            dispatch(&weak, |app, _| app.close_settings());
            slint::CloseRequestResponse::KeepWindowShown
        });
        if let Err(error) = ui.show() {
            // Release the window or the app keeps believing settings are open.
            self.settings_ui = None;
            self.color_draft = None;
            return Err(error).context("cannot show the settings window");
        }
        self.track_settings_window();
        Ok(())
    }
}
