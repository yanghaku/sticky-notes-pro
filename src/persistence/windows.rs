//! Per-window state validation and transactional window history writes.

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Row, Transaction, params};

use super::now_millis;

pub(super) const WINDOW_COLUMNS: &str = "id, window_type, note_id, normal_x, normal_y, normal_width, normal_height, monitor_id, scale_factor, maximized, always_on_top, updated_at, last_used_at";

#[derive(Debug, Clone)]
pub struct WindowState {
    pub id: String,
    pub window_type: String,
    pub note_id: Option<String>,
    pub normal_x: Option<f64>,
    pub normal_y: Option<f64>,
    pub normal_width: f64,
    pub normal_height: f64,
    pub monitor_id: Option<String>,
    pub scale_factor: Option<f64>,
    pub maximized: bool,
    pub always_on_top: bool,
    pub updated_at: i64,
    pub last_used_at: i64,
}

impl WindowState {
    pub fn for_note(note_id: &str, width: f64, height: f64) -> Self {
        let mut state = Self::for_aux("note", width, height);
        state.id = format!("note:{note_id}");
        state.note_id = Some(note_id.to_owned());
        state
    }

    pub fn for_aux(kind: &str, width: f64, height: f64) -> Self {
        let now = now_millis();
        Self {
            id: kind.to_owned(),
            window_type: kind.to_owned(),
            note_id: None,
            normal_x: None,
            normal_y: None,
            normal_width: width,
            normal_height: height,
            monitor_id: None,
            scale_factor: None,
            maximized: false,
            always_on_top: false,
            updated_at: now,
            last_used_at: now,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.normal_width.is_finite() && self.normal_width > 0.0,
            "invalid normal window width"
        );
        ensure!(
            self.normal_height.is_finite() && self.normal_height > 0.0,
            "invalid normal window height"
        );
        ensure!(
            self.normal_x.is_some() == self.normal_y.is_some(),
            "window coordinates must be present together"
        );
        ensure!(
            self.normal_x.is_none_or(f64::is_finite) && self.normal_y.is_none_or(f64::is_finite),
            "window coordinates must be finite"
        );
        ensure!(
            self.scale_factor
                .is_none_or(|scale| scale.is_finite() && scale > 0.0),
            "invalid window scale factor"
        );
        match (self.window_type.as_str(), self.note_id.as_deref()) {
            ("note", Some(note_id)) => ensure!(
                self.id == format!("note:{note_id}"),
                "invalid note window identity"
            ),
            ("notes_list" | "settings", None) => ensure!(
                self.id == self.window_type,
                "invalid auxiliary window identity"
            ),
            _ => bail!("invalid window type or note association"),
        }
        Ok(())
    }
}

pub(super) fn read_window(row: &Row<'_>) -> rusqlite::Result<WindowState> {
    Ok(WindowState {
        id: row.get(0)?,
        window_type: row.get(1)?,
        note_id: row.get(2)?,
        normal_x: row.get(3)?,
        normal_y: row.get(4)?,
        normal_width: row.get(5)?,
        normal_height: row.get(6)?,
        monitor_id: row.get(7)?,
        scale_factor: row.get(8)?,
        maximized: row.get(9)?,
        always_on_top: row.get(10)?,
        updated_at: row.get(11)?,
        last_used_at: row.get(12)?,
    })
}

pub(super) fn save_window_in(transaction: &Transaction<'_>, state: &WindowState) -> Result<()> {
    state.validate()?;
    let written = transaction.execute(
        "INSERT INTO window_states (id, window_type, note_id, normal_x, normal_y, normal_width, normal_height, monitor_id, scale_factor, maximized, always_on_top, updated_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET normal_x = excluded.normal_x, normal_y = excluded.normal_y,
         normal_width = excluded.normal_width, normal_height = excluded.normal_height, monitor_id = excluded.monitor_id,
         scale_factor = excluded.scale_factor, maximized = excluded.maximized, always_on_top = excluded.always_on_top,
         updated_at = excluded.updated_at, last_used_at = excluded.last_used_at
         WHERE window_states.updated_at <= excluded.updated_at",
        params![state.id, state.window_type, state.note_id, state.normal_x, state.normal_y, state.normal_width, state.normal_height, state.monitor_id, state.scale_factor, state.maximized, state.always_on_top, state.updated_at, state.last_used_at],
    );
    // A rejected row means a newer state already won; reporting success there
    // would silently discard the caller's geometry and pin changes.
    let changed = written.context("cannot save window state")?;
    ensure!(changed == 1, "refusing stale window state for {}", state.id);
    Ok(())
}

pub(super) fn close_window_in(transaction: &Transaction<'_>, state: &WindowState) -> Result<()> {
    save_window_in(transaction, state)?;
    record_close_in(transaction, state)
}

pub(super) fn record_close_in(transaction: &Transaction<'_>, state: &WindowState) -> Result<()> {
    transaction.execute(
        "INSERT INTO app_state (id, last_closed_window_type, last_closed_note_id, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET last_closed_window_type = excluded.last_closed_window_type,
         last_closed_note_id = excluded.last_closed_note_id, updated_at = excluded.updated_at",
        params![state.window_type, state.note_id, now_millis()],
    ).context("cannot save last closed window")?;
    Ok(())
}
