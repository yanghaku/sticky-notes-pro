//! Deduplicate native IME requests independently of editor presentation.

use super::*;

#[derive(Clone, Copy, PartialEq)]
struct CursorArea {
    x: f32,
    y: f32,
    height: f32,
    scale: f32,
}

#[derive(Default)]
pub(super) struct ImeState {
    allowed: Option<bool>,
    cursor: Option<CursorArea>,
    submitted: Option<CursorArea>,
}

impl ImeState {
    fn set_enabled(&mut self, window: &slint::Window, enabled: bool) {
        if self.allowed == Some(enabled) || !window.has_winit_window() {
            return;
        }
        self.allowed = Some(enabled);
        self.submitted = None;
        platform::enable_ime(window, enabled);
        if enabled {
            self.submit_cursor(window);
        }
    }

    /// A new native text-input session needs its rectangle even when unchanged.
    pub(super) fn enabled(&mut self, window: &slint::Window) {
        self.submitted = None;
        self.submit_cursor(window);
    }

    pub(super) fn disabled(&mut self) {
        self.submitted = None;
    }

    pub(super) fn update_cursor(&mut self, window: &slint::Window, x: f32, y: f32, height: f32) {
        if !x.is_finite() || !y.is_finite() || !height.is_finite() {
            return;
        }
        // Candidate rectangles do not need subpixel precision. Exact equality
        // on these rounded coordinates and the native scale is intentional.
        self.cursor = Some(CursorArea {
            x: x.round(),
            y: y.round(),
            height: height.max(1.0).round(),
            scale: window.scale_factor(),
        });
        self.submit_cursor(window);
    }

    fn submit_cursor(&mut self, window: &slint::Window) {
        if self.allowed != Some(true) || !window.has_winit_window() {
            return;
        }
        if let Some(cursor) = self.cursor
            && self.submitted != Some(cursor)
        {
            platform::set_ime_cursor_area(window, cursor.x, cursor.y, cursor.height);
            self.submitted = Some(cursor);
        }
    }
}

impl Panel {
    pub(super) fn set_ime_enabled(&mut self, enabled: bool) {
        self.ime.set_enabled(self.ui.window(), enabled);
    }
}
