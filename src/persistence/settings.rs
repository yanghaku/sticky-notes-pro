//! Validated preferences and atomic replacement of settings files.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{ErrorKind, Write},
    ops::RangeInclusive,
    path::Path,
};
use tempfile::NamedTempFile;

use super::{NoteColors, paths::parent_directory};
use crate::model::THEMES;

const SETTINGS_VERSION: u32 = 1;

/// Control ranges shared by loading and validation: a value repaired on load
/// must also be one validate() accepts, so neither step may drift alone.
const SCALE_PERCENT: RangeInclusive<u16> = 50..=250;
const MIN_NOTE_WIDTH: f64 = 320.0;
const MIN_NOTE_HEIGHT: f64 = 240.0;
const EDITOR_FONT_SIZE: RangeInclusive<f64> = 10.0..=32.0;
const WINDOW_FONT_SIZE: RangeInclusive<f64> = 10.0..=24.0;
const MAX_FONT_FAMILY: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub note: NoteSettings,
    pub editor: EditorSettings,
    pub window_font: WindowFontSettings,
    pub appearance_mode: AppearanceMode,
    /// Application zoom, multiplied by the system DPI rather than replacing it.
    pub scale_percent: u16,
}

/// The preference is independent of both the OS theme and each note's color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    pub fn index(self) -> i32 {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    pub fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::System),
            1 => Some(Self::Light),
            2 => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteSettings {
    pub default_width: f64,
    pub default_height: f64,
    pub default_theme: String,
    pub colors: NoteColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub font_size: f64,
    /// Empty means the platform's default sans-serif family.
    pub font_family: String,
}

/// UI typography is independent of note-body typography and native window decorations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFontSettings {
    pub font_size: f64,
    /// Empty follows Slint's platform default font.
    pub font_family: String,
}

/// Unreadable preferences do not block notes, but must never be overwritten with defaults.
#[derive(Debug)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub warning: Option<String>,
    pub writable: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_VERSION,
            note: NoteSettings::default(),
            editor: EditorSettings::default(),
            window_font: WindowFontSettings::default(),
            appearance_mode: AppearanceMode::default(),
            scale_percent: 100,
        }
    }
}

impl Default for NoteSettings {
    fn default() -> Self {
        Self {
            default_width: 360.0,
            default_height: 420.0,
            default_theme: "white".to_owned(),
            colors: NoteColors::default(),
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            font_family: String::new(),
        }
    }
}

impl Default for WindowFontSettings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: String::new(),
        }
    }
}

impl Settings {
    pub fn scale_factor(&self) -> f32 {
        f32::from(self.scale_percent) / 100.0
    }

    pub fn load(path: &Path) -> Result<LoadedSettings> {
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(LoadedSettings {
                    settings: Self::default(),
                    warning: None,
                    writable: true,
                });
            }
            Err(error) => return Err(error).context("cannot read preferences"),
        };
        let value = match serde_json::from_slice::<Value>(&contents) {
            Ok(Value::Object(object)) => Value::Object(object),
            Ok(_) => return Ok(Self::unreadable("preferences must be a JSON object")),
            Err(error) => {
                return Ok(Self::unreadable(&format!(
                    "cannot parse preferences: {error}"
                )));
            }
        };
        if let Some(version) = value.get("schema_version")
            && version.as_u64() != Some(u64::from(SETTINGS_VERSION))
        {
            return Ok(Self::unreadable(
                "preferences use an unsupported schema version",
            ));
        }

        // Read each known preference independently so one bad value cannot discard the others.
        let mut settings = Self::default();
        let mut repaired = false;
        settings.note.colors =
            NoteColors::read_preferences(value.pointer("/note/colors"), &mut repaired);
        if let Some(scale) = value.get("scale_percent") {
            match scale
                .as_u64()
                .and_then(|scale| u16::try_from(scale).ok())
                .filter(|scale| SCALE_PERCENT.contains(scale))
            {
                Some(scale) => settings.scale_percent = scale,
                None => repaired = true,
            }
        }
        settings.note.default_width = preference_number(
            &value,
            "/note/default_width",
            settings.note.default_width,
            |number| number >= MIN_NOTE_WIDTH,
            &mut repaired,
        );
        settings.note.default_height = preference_number(
            &value,
            "/note/default_height",
            settings.note.default_height,
            |number| number >= MIN_NOTE_HEIGHT,
            &mut repaired,
        );
        settings.editor.font_size = preference_number(
            &value,
            "/editor/font_size",
            settings.editor.font_size,
            |number| EDITOR_FONT_SIZE.contains(&number),
            &mut repaired,
        );
        settings.editor.font_family =
            preference_family(&value, "/editor/font_family", &mut repaired);
        settings.window_font.font_family =
            preference_family(&value, "/window_font/font_family", &mut repaired);
        settings.window_font.font_size = preference_number(
            &value,
            "/window_font/font_size",
            settings.window_font.font_size,
            |number| WINDOW_FONT_SIZE.contains(&number),
            &mut repaired,
        );
        if let Some(theme) = value.pointer("/note/default_theme") {
            match theme.as_str().filter(|theme| known_theme(theme)) {
                Some(theme) => settings.note.default_theme = theme.to_owned(),
                None => repaired = true,
            }
        }
        if let Some(mode) = value.get("appearance_mode") {
            match mode.as_str() {
                Some("system") => settings.appearance_mode = AppearanceMode::System,
                Some("light") => settings.appearance_mode = AppearanceMode::Light,
                Some("dark") => settings.appearance_mode = AppearanceMode::Dark,
                _ => repaired = true,
            }
        }
        for group in ["note", "editor", "window_font"] {
            if let Some(group) = value.get(group)
                && !group.is_object()
            {
                repaired = true;
            }
        }
        Ok(LoadedSettings {
            settings,
            warning: repaired
                .then(|| "invalid preference values were replaced with defaults".to_owned()),
            writable: true,
        })
    }

    fn unreadable(reason: &str) -> LoadedSettings {
        LoadedSettings {
            settings: Self::default(),
            warning: Some(format!("{reason}; original file is preserved")),
            writable: false,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SETTINGS_VERSION,
            "unsupported preferences version"
        );
        ensure!(
            SCALE_PERCENT.contains(&self.scale_percent),
            "application scale must be between 50 and 250 percent"
        );
        ensure!(
            self.note.default_width.is_finite() && self.note.default_width >= MIN_NOTE_WIDTH,
            "default width must be at least 320 logical pixels"
        );
        ensure!(
            self.note.default_height.is_finite() && self.note.default_height >= MIN_NOTE_HEIGHT,
            "default height must be at least 240 logical pixels"
        );
        ensure!(
            self.editor.font_size.is_finite() && EDITOR_FONT_SIZE.contains(&self.editor.font_size),
            "font size must be between 10 and 32"
        );
        ensure!(
            self.editor.font_family.trim().chars().count() <= MAX_FONT_FAMILY,
            "font family must be at most 200 characters"
        );
        ensure!(
            self.window_font.font_size.is_finite()
                && WINDOW_FONT_SIZE.contains(&self.window_font.font_size),
            "window font size must be between 10 and 24"
        );
        ensure!(
            self.window_font.font_family.trim().chars().count() <= MAX_FONT_FAMILY,
            "window font family must be at most 200 characters"
        );
        ensure!(
            known_theme(&self.note.default_theme),
            "unknown default theme"
        );
        Ok(())
    }

    /// Replace a complete preferences file atomically; damaged or future files remain untouched.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        ensure!(
            Self::load(path)?.writable,
            "cannot overwrite unreadable preferences; preserve or move the original file first"
        );
        let directory = parent_directory(path);
        fs::create_dir_all(directory).context("cannot create preferences directory")?;
        let mut temporary =
            NamedTempFile::new_in(directory).context("cannot create preferences temporary file")?;
        let mut normalized = self.clone();
        normalized.editor.font_family = normalized.editor.font_family.trim().to_owned();
        normalized.window_font.font_family = normalized.window_font.font_family.trim().to_owned();
        serde_json::to_writer_pretty(&mut temporary, &normalized)
            .context("cannot serialize preferences")?;
        temporary
            .write_all(b"\n")
            .context("cannot finish preferences file")?;
        temporary
            .as_file()
            .sync_all()
            .context("cannot sync preferences")?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .context("cannot atomically replace preferences")?;
        // The rename already replaced the file: a later directory-sync failure must
        // not be reported as a failed save, or the caller keeps the old settings in
        // memory while the file on disk already holds the new ones.
        #[cfg(unix)]
        if let Err(error) = fs::File::open(directory).and_then(|directory| directory.sync_all()) {
            eprintln!(
                "preferences were replaced without syncing {}: {error}",
                directory.display()
            );
        }
        Ok(())
    }
}

fn preference_family(value: &Value, pointer: &str, repaired: &mut bool) -> String {
    let Some(value) = value.pointer(pointer) else {
        return String::new();
    };
    match value
        .as_str()
        .map(str::trim)
        .filter(|family| family.chars().count() <= MAX_FONT_FAMILY)
    {
        Some(family) => family.to_owned(),
        None => {
            *repaired = true;
            String::new()
        }
    }
}

fn preference_number(
    value: &Value,
    pointer: &str,
    fallback: f64,
    valid: impl FnOnce(f64) -> bool,
    repaired: &mut bool,
) -> f64 {
    let Some(value) = value.pointer(pointer) else {
        return fallback;
    };
    match value
        .as_f64()
        .filter(|number| number.is_finite() && valid(*number))
    {
        Some(number) => number,
        None => {
            *repaired = true;
            fallback
        }
    }
}

fn known_theme(theme: &str) -> bool {
    THEMES.contains(&theme)
}
