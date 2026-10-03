//! Window operations with explicit capability detection for X11 and Wayland.

use slint::winit_030::{WinitWindowAccessor, winit};
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::WindowLevel;

use crate::persistence::{WindowState, now_millis};

#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub position: bool,
    pub pin: bool,
    pub is_wayland: bool,
}

fn native_capabilities(window: &winit::window::Window) -> Capabilities {
    match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Wayland(_)) => Capabilities {
            is_wayland: true,
            ..Capabilities::default()
        },
        Ok(
            RawWindowHandle::Win32(_)
            | RawWindowHandle::Xlib(_)
            | RawWindowHandle::Xcb(_)
            | RawWindowHandle::AppKit(_),
        ) => Capabilities {
            position: true,
            pin: true,
            is_wayland: false,
        },
        _ => Capabilities::default(),
    }
}

pub fn capabilities(window: &slint::Window) -> Capabilities {
    window
        .with_winit_window(native_capabilities)
        .unwrap_or_default()
}

fn monitor_for(
    window: &winit::window::Window,
    hint: Option<&str>,
) -> Option<winit::monitor::MonitorHandle> {
    hint.and_then(|name| {
        window
            .available_monitors()
            .find(|monitor| monitor.name().as_deref() == Some(name))
    })
    .or_else(|| window.current_monitor())
    .or_else(|| window.primary_monitor())
    .or_else(|| window.available_monitors().next())
}

fn monitor_size(monitor: &winit::monitor::MonitorHandle) -> LogicalSize<f64> {
    let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
    // Winit exposes monitor dimensions, not a portable desktop work area.
    // Keep the same decoration/panel allowance used by restoration.
    LogicalSize::new(size.width.max(1.0), (size.height - 48.0).max(1.0))
}

/// Bound zoomed note minima in the same logical units as window restoration.
/// A missing monitor leaves the UI's conservative initial minima unchanged.
pub fn available_size(window: &slint::Window, hint: Option<&str>) -> Option<slint::LogicalSize> {
    window
        .with_winit_window(|native| {
            let monitor = monitor_for(native, hint)?;
            let size = monitor_size(&monitor);
            Some(slint::LogicalSize::new(
                size.width as f32,
                size.height as f32,
            ))
        })
        .flatten()
}

/// Apply the normal rectangle before maximize and pin. Call after showing the window.
pub fn restore(window: &slint::Window, state: &WindowState) {
    window.with_winit_window(|native| {
        let caps = native_capabilities(native);
        let monitor = monitor_for(native, state.monitor_id.as_deref());
        let scale = monitor
            .as_ref()
            .map_or(native.scale_factor(), |monitor| monitor.scale_factor());
        let available = monitor
            .as_ref()
            .map_or(LogicalSize::new(3840.0, 2112.0), monitor_size);
        let width = positive(state.normal_width, 420.0)
            .clamp(320.0_f64.min(available.width), available.width);
        let height = positive(state.normal_height, 480.0)
            .clamp(240.0_f64.min(available.height), available.height);
        let _ = native.request_inner_size(LogicalSize::new(width, height));
        if caps.position
            && let Some(monitor) = monitor
        {
            let origin = monitor.position();
            let size = monitor.size();
            let outer = native.outer_size();
            let inner = native.inner_size();
            let frame_width = outer.width.saturating_sub(inner.width);
            let frame_height = outer.height.saturating_sub(inner.height);
            let restored = LogicalSize::new(width, height).to_physical::<u32>(scale);
            let full_width = restored.width.saturating_add(frame_width).min(size.width);
            let full_height = restored
                .height
                .saturating_add(frame_height)
                .min(size.height);
            let left = f64::from(origin.x);
            let top = f64::from(origin.y);
            let right = left + f64::from(size.width.saturating_sub(full_width));
            let bottom = top + f64::from(size.height.saturating_sub(full_height));
            let x = state
                .normal_x
                .filter(|x| x.is_finite())
                .map_or(left + 24.0 * scale, |x| x * scale)
                .clamp(left, right);
            let y = state
                .normal_y
                .filter(|y| y.is_finite())
                .map_or(top + 24.0 * scale, |y| y * scale)
                .clamp(top, bottom);
            native.set_outer_position(PhysicalPosition::new(
                round_coordinate(x),
                round_coordinate(y),
            ));
        }
        native.set_maximized(state.maximized);
        if caps.pin {
            native.set_window_level(if state.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
        }
    });
}

/// Capture only a normal, nonminimized rectangle; preserve it while maximized.
pub fn capture(window: &slint::Window, previous: &WindowState) -> WindowState {
    window
        .with_winit_window(|native| {
            let mut state = previous.clone();
            let scale = native.scale_factor();
            state.maximized = native.is_maximized();
            state.scale_factor = Some(scale);
            state.monitor_id = native.current_monitor().and_then(|monitor| monitor.name());
            if !state.maximized && native.is_minimized() != Some(true) {
                let size = native.inner_size().to_logical::<f64>(scale);
                if size.width > 0.0 && size.height > 0.0 {
                    state.normal_width = size.width;
                    state.normal_height = size.height;
                }
                if native_capabilities(native).position
                    && let Ok(position) = native.outer_position()
                {
                    let position = position.to_logical::<f64>(scale);
                    state.normal_x = Some(position.x);
                    state.normal_y = Some(position.y);
                }
            }
            // Keep the stamp strictly newer than the value read from the
            // database: the upsert below rejects anything older, and a clock
            // step backwards across sessions would otherwise drop the change.
            state.updated_at = now_millis().max(previous.updated_at.saturating_add(1));
            state
        })
        .unwrap_or_else(|| previous.clone())
}

pub fn activate(window: &slint::Window) {
    window.with_winit_window(|native| {
        native.set_minimized(false);
        native.focus_window();
    });
}

pub fn drag(window: &slint::Window) -> Result<(), String> {
    window
        .with_winit_window(|native| native.drag_window().map_err(|error| error.to_string()))
        .unwrap_or_else(|| Err("window dragging is unavailable".into()))
}

/// Candidate rectangle uses logical window coordinates, including editor offsets.
/// It never changes whether the IME is enabled; the caller owns that lifecycle.
pub fn set_ime_cursor_area(window: &slint::Window, x: f32, y: f32, line_height: f32) {
    if !x.is_finite() || !y.is_finite() || !line_height.is_finite() {
        return;
    }
    window.with_winit_window(|native| {
        native.set_ime_cursor_area(
            LogicalPosition::new(f64::from(x), f64::from(y)),
            LogicalSize::new(1.0, f64::from(line_height.max(1.0))),
        );
    });
}

pub fn enable_ime(window: &slint::Window, enabled: bool) {
    window.with_winit_window(|native| native.set_ime_allowed(enabled));
}

/// Access the backend already owning this window and its native display.
///
/// Slint 1.17.1 does not expose this through its public Window API. Keep this
/// version-sensitive bridge here, and recheck it when upgrading the pinned Slint
/// dependency. No raw pointer or second Wayland connection is created here.
fn clipboard_platform(window: &slint::Window) -> anyhow::Result<&dyn slint::platform::Platform> {
    use anyhow::Context;
    use slint::private_unstable_api::re_exports::WindowInner;

    WindowInner::from_pub(window)
        .try_context()
        .map(|context| context.platform())
        .context("window backend is unavailable")
}

/// Submit a copy request using the backend's normal clipboard.
///
/// Success means the request reached Slint, not that the compositor acknowledged
/// it: Slint's Platform setter does not expose native write errors. On Wayland
/// call from user input while the application has keyboard focus.
pub fn set_clipboard_text(window: &slint::Window, text: &str) -> anyhow::Result<()> {
    clipboard_platform(window)?
        .set_clipboard_text(text, slint::platform::Clipboard::DefaultClipboard);
    Ok(())
}

/// Read UTF-8 text from the backend's normal clipboard, not primary selection.
pub fn clipboard_text(window: &slint::Window) -> anyhow::Result<Option<String>> {
    Ok(clipboard_platform(window)?.clipboard_text(slint::platform::Clipboard::DefaultClipboard))
}

fn positive(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn round_coordinate(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
