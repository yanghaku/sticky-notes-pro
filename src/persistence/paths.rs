//! Platform-specific locations for the application's durable files.

use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Paths are separated on ordinary installs; the override keeps everything in one directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub settings_file: PathBuf,
    pub database_file: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let (data_dir, config_dir) = match std::env::var_os("STICKY_NOTES_DATA_DIR") {
            Some(path) => {
                ensure!(!path.is_empty(), "STICKY_NOTES_DATA_DIR cannot be empty");
                let directory = PathBuf::from(path);
                (directory.clone(), directory)
            }
            None => {
                let directories = ProjectDirs::from("", "", "sticky-notes-pro")
                    .context("cannot locate application directories")?;
                (
                    directories.data_local_dir().to_owned(),
                    directories.config_dir().to_owned(),
                )
            }
        };
        fs::create_dir_all(&data_dir).context("cannot create application data directory")?;
        fs::create_dir_all(&config_dir).context("cannot create application config directory")?;
        Ok(Self {
            settings_file: config_dir.join("settings.json"),
            database_file: data_dir.join("app.db"),
            data_dir,
        })
    }
}

pub(super) fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
