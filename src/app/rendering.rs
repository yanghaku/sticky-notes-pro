//! Frame-driven editor presentation with layout-only input queries.

use super::*;
use slint::winit_030::CustomApplicationHandler;

pub(super) struct RenderCoordinator(pub(super) Weak<RefCell<App>>);

impl CustomApplicationHandler for RenderCoordinator {
    fn about_to_wait(&mut self, _: &winit::event_loop::ActiveEventLoop) -> EventResult {
        if let Some(shared) = self.0.upgrade()
            && let Ok(app) = shared.try_borrow()
        {
            // Slint may flush a delayed pointer move between our redraw filter
            // and draw(). Re-arm any resulting edits after the frame completes.
            app.schedule_render();
        }
        EventResult::Propagate
    }
}

impl App {
    /// The backend coalesces requests; the native redraw hook paints the latest
    /// document before Slint composites that same frame.
    pub(super) fn schedule_render(&self) {
        for panel in self.notes.values() {
            if panel.restored && panel.render_dirty && panel.editor.is_some() {
                panel.ui.window().request_redraw();
            }
        }
    }
}

impl Panel {
    fn update_view(&mut self) -> (f32, f32, f32) {
        // Layout/scroll/hit coordinates remain in unzoomed editor units. Raster
        // at system DPI × app zoom, subject to the existing raster memory budget.
        let app_scale = self.ui.global::<AppAppearance>().get_ui_scale();
        let width = (self.ui.get_view_width() / app_scale).max(1.0);
        let height = (self.ui.get_view_height() / app_scale).max(1.0);
        let scale = self.ui.window().scale_factor() * app_scale;
        if self.view != (width, height, scale) {
            self.render_dirty = true;
            self.view = (width, height, scale);
        }
        self.view
    }

    /// Navigation and hit-testing need current geometry, not a new image.
    pub(super) fn refresh_layout(&mut self, font_size: f32) {
        if !self.restored {
            return;
        }
        let (width, height, scale) = self.update_view();
        if !self.render_dirty {
            return;
        }
        let Some(editor) = &self.editor else { return };
        let prepared = self.layout.prepare(
            &editor.rows(),
            editor.selection(),
            &self.preedit,
            width,
            height,
            font_size,
            scale,
            self.scroll,
            self.follow_caret,
        );
        self.scroll = prepared.scroll;
        // Keep render_dirty/follow_caret until the native redraw presents the
        // result. A geometry query must not consume a pending visual update.
    }

    pub(super) fn paint(&mut self, font_size: f32) {
        if !self.restored {
            return;
        }
        let (width, height, scale) = self.update_view();
        if !self.render_dirty {
            return;
        }
        let Some(editor) = &self.editor else { return };
        let rows = editor.rows();
        let rendered = self.layout.render(
            &rows,
            editor.selection(),
            &self.preedit,
            width,
            height,
            font_size,
            scale,
            self.scroll,
            self.follow_caret,
        );
        self.scroll = rendered.scroll;
        self.ui.set_editor_image(rendered.image);
        let app_scale = self.ui.global::<AppAppearance>().get_ui_scale();
        self.ime.update_cursor(
            self.ui.window(),
            self.ui.get_canvas_x() + rendered.caret.0 * app_scale,
            self.ui.get_canvas_y() + rendered.caret.1 * app_scale,
            font_size * 1.5 * app_scale,
        );
        self.render_dirty = false;
        self.follow_caret = false;
    }
}
