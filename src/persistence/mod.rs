//! Local SQLite storage, atomic preferences, and per-window application state.

mod paths;
mod repository;
mod schema;
mod settings;
mod windows;

pub use paths::Paths;
pub use repository::{Repository, StoredNote};
pub use settings::{AppearanceMode, Settings};
pub use windows::WindowState;

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, shared by model and window write operations.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
