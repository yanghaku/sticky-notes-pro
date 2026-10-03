//! Global appearance propagation.

use super::*;

impl App {
    pub(super) fn configure_appearance<'a, C: ComponentHandle>(&self, ui: &'a C)
    where
        AppAppearance<'a>: slint::Global<'a, C>,
    {
        let appearance = ui.global::<AppAppearance>();
        appearance.invoke_apply_mode(self.settings.appearance_mode.index());
        appearance.set_font_family(self.settings.window_font.font_family.as_str().into());
        appearance.set_font_size(self.settings.window_font.font_size as f32);
        appearance.set_scale_percent(i32::from(self.settings.scale_percent));
        let colors = &self.settings.note.colors;
        appearance.set_light_note_colors(colors::palette_model(&colors.light));
        appearance.set_dark_note_colors(colors::palette_model(&colors.dark));
        appearance.set_light_note_border(colors::ui_color(colors.light.border));
        appearance.set_dark_note_border(colors::ui_color(colors.dark.border));
    }

    pub(super) fn apply_appearance(&mut self) {
        for panel in self.notes.values() {
            self.configure_appearance(&panel.ui);
        }
        if let Some(ui) = &self.list {
            self.configure_appearance(ui);
        }
        if let Some(ui) = &self.settings_ui {
            self.configure_appearance(ui);
        }
        if let Some(ui) = &self.discard {
            self.configure_appearance(ui);
        }
        if let Some((_, ui)) = &self.permanent_dialog {
            self.configure_appearance(ui);
        }
        self.dark = None;
        self.sync_appearance();
    }

    /// Observe effective system appearance without querying the database.
    pub(super) fn sync_appearance(&mut self) {
        let dark = self
            .notes
            .values()
            .next()
            .map(|panel| panel.ui.global::<AppAppearance>().get_dark())
            .or_else(|| {
                self.list
                    .as_ref()
                    .map(|ui| ui.global::<AppAppearance>().get_dark())
            })
            .or_else(|| {
                self.settings_ui
                    .as_ref()
                    .map(|ui| ui.global::<AppAppearance>().get_dark())
            })
            .or_else(|| {
                self.permanent_dialog
                    .as_ref()
                    .map(|(_, ui)| ui.global::<AppAppearance>().get_dark())
            });
        if dark == self.dark {
            return;
        }
        self.dark = dark;
        let Some(dark) = dark else { return };
        for panel in self.notes.values_mut() {
            panel.layout.set_dark(dark);
            panel.render_dirty = true;
        }
        self.list_dirty_rows.extend(self.list_rows.keys().cloned());
        self.list_dirty = true;
        self.list_next_refresh = Instant::now();
    }
}
