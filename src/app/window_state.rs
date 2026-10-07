//! Bounded window-state submission timing shared by note and auxiliary windows.

use super::*;

const STATE_DEBOUNCE: Duration = Duration::from_millis(400);
const STATE_INTERVAL: Duration = Duration::from_secs(2);

impl Panel {
    pub(super) fn update_window_limits(&self, monitor_hint: Option<&str>) {
        let window = self.ui.window();
        let Some(available) = platform::available_size(window, monitor_hint) else {
            return;
        };
        if self.ui.get_available_width() == available.width
            && self.ui.get_available_height() == available.height
        {
            return;
        }
        platform::update_note_window(window, || {
            self.ui.set_available_width(available.width);
            self.ui.set_available_height(available.height);
            if !window.is_maximized() && !window.is_minimized() {
                let size = window.size().to_logical(window.scale_factor());
                if size.width > available.width || size.height > available.height {
                    window.set_size(slint::LogicalSize::new(
                        size.width.min(available.width),
                        size.height.min(available.height),
                    ));
                }
            }
        });
    }
}

#[derive(Default)]
pub(super) struct WindowSaveTimer {
    pending_since: Option<Instant>,
    retry_after: Option<Instant>,
}

impl WindowSaveTimer {
    pub(super) fn observe(&mut self, latest_change: Option<Instant>) {
        if let Some(latest_change) = latest_change {
            self.pending_since.get_or_insert(latest_change);
        } else {
            self.pending_since = None;
        }
    }

    pub(super) fn ready(&self, latest_change: Option<Instant>, now: Instant) -> bool {
        latest_change.is_some_and(|latest| {
            self.retry_after.is_none_or(|deadline| now >= deadline)
                && (now.saturating_duration_since(latest) >= STATE_DEBOUNCE
                    || self.pending_since.is_some_and(|first| {
                        now.saturating_duration_since(first) >= STATE_INTERVAL
                    }))
        })
    }

    pub(super) fn submitted(&mut self) {
        self.pending_since = None;
        self.retry_after = None;
    }

    pub(super) fn failed(&mut self, now: Instant) {
        self.pending_since.get_or_insert(now);
        self.retry_after = Some(now + STATE_INTERVAL);
    }
}

pub(super) struct AuxiliaryWindowState {
    pub(super) state: WindowState,
    dirty: Option<Instant>,
    in_flight: bool,
    /// The banner owned by the last failed window-state submission.
    failure_message: Option<slint::SharedString>,
    timer: WindowSaveTimer,
    restore_started: Option<Instant>,
}

impl AuxiliaryWindowState {
    fn new(state: WindowState) -> Self {
        Self {
            state,
            dirty: None,
            in_flight: false,
            failure_message: None,
            timer: WindowSaveTimer::default(),
            restore_started: None,
        }
    }

    fn maintain(&mut self, window: &slint::Window, worker: &Worker, now: Instant) -> Result<()> {
        if !window.has_winit_window() {
            return Ok(());
        }
        let latest = platform::capture(window, &self.state);
        if let Some(start) = self.restore_started {
            let elapsed = now.saturating_duration_since(start);
            if elapsed.is_zero()
                || (self.state.maximized && !latest.maximized && elapsed < STATE_INTERVAL)
            {
                // Resize/maximize requests may not be applied until the next
                // native event. Keep the saved normal rectangle in the interim.
                return Ok(());
            }
            self.restore_started = None;
        }
        if rectangle_changed(&self.state, &latest) {
            self.state = latest;
            self.dirty = Some(now);
        }
        self.timer.observe(self.dirty);
        if !self.in_flight && self.timer.ready(self.dirty, now) {
            match worker.submit(Job::SaveWindow {
                state: self.state.clone(),
            }) {
                Ok(()) => {
                    self.dirty = None;
                    self.in_flight = true;
                    self.timer.submitted();
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                Err(error) => {
                    self.timer.failed(now);
                    return Err(anyhow::anyhow!("save worker unavailable: {error}"));
                }
            }
        }
        Ok(())
    }

    /// Return the previous failure banner so success clears only its own message.
    pub(super) fn completed(
        &mut self,
        failure_message: Option<slint::SharedString>,
    ) -> Option<slint::SharedString> {
        let previous_message = std::mem::replace(&mut self.failure_message, failure_message);
        self.in_flight = false;
        if self.failure_message.is_some() {
            let now = Instant::now();
            // Retry the current rectangle, not the older failed snapshot. Changes
            // observed while that snapshot was queued remain pending as well.
            self.dirty.get_or_insert(now);
            self.timer.failed(now);
        }
        previous_message
    }

    pub(super) fn restoring(&mut self, now: Instant) {
        self.restore_started = Some(now);
    }
}

impl App {
    pub(super) fn track_list_window(&mut self) {
        self.list_state = Some(AuxiliaryWindowState::new(
            self.restore_list
                .clone()
                .unwrap_or_else(|| WindowState::for_aux("notes_list", 620.0, 520.0)),
        ));
    }

    pub(super) fn track_settings_window(&mut self) {
        self.settings_state = Some(AuxiliaryWindowState::new(
            self.restore_settings
                .clone()
                .unwrap_or_else(|| WindowState::for_aux("settings", 420.0, 530.0)),
        ));
    }

    pub(super) fn maintain_auxiliary_windows(&mut self, now: Instant) -> Result<()> {
        if !self.closing_list
            && let (Some(ui), Some(tracked)) = (&self.list, &mut self.list_state)
        {
            tracked.maintain(ui.window(), &self.worker, now)?;
        }
        if !self.closing_settings
            && let (Some(ui), Some(tracked)) = (&self.settings_ui, &mut self.settings_state)
        {
            tracked.maintain(ui.window(), &self.worker, now)?;
        }
        Ok(())
    }
}

fn rectangle_changed(old: &WindowState, new: &WindowState) -> bool {
    fn different(a: f64, b: f64) -> bool {
        (a - b).abs() > 0.000_001
    }
    fn optional_different(a: Option<f64>, b: Option<f64>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => different(a, b),
            (None, None) => false,
            _ => true,
        }
    }
    different(old.normal_width, new.normal_width)
        || different(old.normal_height, new.normal_height)
        || optional_different(old.normal_x, new.normal_x)
        || optional_different(old.normal_y, new.normal_y)
        || optional_different(old.scale_factor, new.scale_factor)
        || old.monitor_id != new.monitor_id
        || old.maximized != new.maximized
        || old.always_on_top != new.always_on_top
}
