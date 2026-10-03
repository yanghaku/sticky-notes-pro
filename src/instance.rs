//! A process-lifetime filesystem lock and a local request to activate the Notes List.

use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use fs2::FileExt;

/// Keep this guard alive until all database users and the persistence worker have exited.
pub struct Instance {
    _lock: File,
    activation: PathBuf,
}

impl Instance {
    /// None means another writer owns the lock and received an activation request.
    pub fn acquire(data_dir: &Path) -> Result<Option<Self>> {
        fs::create_dir_all(data_dir).context("cannot create instance lock directory")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join("instance.lock"))
            .context("cannot open application instance lock")?;
        let activation = data_dir.join("activation.request");
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => {
                // Do not clear requests here: another process can request activation as soon
                // as the lock is acquired, before UI initialization and its first poll.
                Ok(Some(Self {
                    _lock: lock,
                    activation,
                }))
            }
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                // Existence is the signal. Concurrent requests intentionally coalesce into
                // one activation; opening an already-open Notes List is the same action.
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(activation)
                    .context("cannot send activation request to the running application")?;
                Ok(None)
            }
            Err(error) => Err(error).context("cannot acquire application instance lock"),
        }
    }

    /// Remove the exact request file; arriving requests remain for the next UI poll.
    pub fn take_activation(&self) -> Result<bool> {
        match fs::remove_file(&self.activation) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("cannot consume application activation request"),
        }
    }
}
