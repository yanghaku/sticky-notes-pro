//! A bounded FIFO persistence queue. The UI retries submissions when the queue is full.

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, ensure};

use crate::{
    model::Note,
    persistence::{Repository, WindowState, now_millis},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAction {
    Save,
    Close,
    Delete,
}

#[derive(Debug)]
pub enum Job {
    Save {
        note: Note,
        state: Option<WindowState>,
        revision: u64,
        action: SaveAction,
        create: bool,
    },
    CloseWindow {
        state: WindowState,
    },
    CloseEmpty {
        id: String,
        state: WindowState,
    },
    SaveWindow {
        state: WindowState,
    },
    Delete {
        id: String,
        state: Option<WindowState>,
    },
    Restore {
        id: String,
    },
    Purge {
        id: String,
    },
}

impl Job {
    /// The reply identity of this job, computed before it is consumed so a
    /// failure while processing can still answer with it.
    fn receipt(&self) -> (String, u64, ReplyKind) {
        match self {
            Job::Save {
                note,
                revision,
                action,
                ..
            } => (
                note.id.clone(),
                *revision,
                match action {
                    SaveAction::Save => ReplyKind::Save,
                    SaveAction::Close => ReplyKind::Close,
                    SaveAction::Delete => ReplyKind::Delete,
                },
            ),
            Job::CloseEmpty { id, .. } => (id.clone(), 0, ReplyKind::EmptyClosed),
            Job::CloseWindow { state } => (state.id.clone(), 0, ReplyKind::Window),
            Job::SaveWindow { state } => (state.id.clone(), 0, ReplyKind::WindowSaved),
            Job::Delete { id, .. } => (id.clone(), 0, ReplyKind::Delete),
            Job::Restore { id } => (id.clone(), 0, ReplyKind::Restore),
            Job::Purge { id } => (id.clone(), 0, ReplyKind::Purge),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    Save,
    Close,
    Delete,
    Restore,
    Window,
    WindowSaved,
    EmptyClosed,
    Purge,
}

#[derive(Debug)]
pub struct Reply {
    pub id: String,
    pub revision: u64,
    pub kind: ReplyKind,
    pub result: std::result::Result<(), String>,
    /// A noncritical window write failed after the content was safely committed.
    pub warning: Option<String>,
}

pub struct Worker {
    sender: Option<SyncSender<Box<Job>>>,
    replies: Receiver<Reply>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Open before spawning so migration and startup failures reach the caller immediately.
    pub fn new(path: &Path) -> Result<Self> {
        let mut repository = Repository::open(path)?;
        let (sender, receiver) = mpsc::sync_channel::<Box<Job>>(32);
        let (reply_sender, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("note-storage".to_owned())
            .spawn(move || {
                let mut clock = WriteClock::default();
                while let Ok(job) = receiver.recv() {
                    let reply = process(&mut repository, &mut clock, *job);
                    // If the receiver was dropped, still drain queued saves before shutdown.
                    let _ = reply_sender.send(reply);
                }
            })
            .context("cannot start persistence worker")?;
        Ok(Self {
            sender: Some(sender),
            replies,
            thread: Some(thread),
        })
    }

    /// Never block the UI. Full/disconnected errors return the untouched job to its caller.
    pub fn submit(&self, job: Job) -> std::result::Result<(), TrySendError<Box<Job>>> {
        match &self.sender {
            Some(sender) => sender.try_send(Box::new(job)),
            None => Err(TrySendError::Disconnected(Box::new(job))),
        }
    }

    pub fn poll(&self) -> Option<Reply> {
        self.replies.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Close the sole sender, then let the worker finish every already accepted command.
        self.sender.take();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("persistence worker terminated unexpectedly");
        }
    }
}

#[derive(Default)]
struct WriteClock {
    notes: HashMap<String, i64>,
    windows: HashMap<String, i64>,
}

impl WriteClock {
    fn note(&mut self, note: &mut Note) {
        let prior = self.notes.entry(note.id.clone()).or_insert(note.updated_at);
        // Content timestamps originate at the edit, not at a later autosave or
        // close. FIFO writes may share a timestamp; the SQL guard permits this.
        note.updated_at = note.updated_at.max(*prior);
        *prior = note.updated_at;
    }

    fn window(&mut self, state: &mut WindowState) {
        let prior = self
            .windows
            .entry(state.id.clone())
            .or_insert(state.updated_at);
        state.updated_at = now_millis()
            .max(state.updated_at)
            .max(prior.saturating_add(1));
        *prior = state.updated_at;
    }
}

fn process(repository: &mut Repository, clock: &mut WriteClock, job: Job) -> Reply {
    let receipt = job.receipt();
    let unanswered = Reply {
        id: receipt.0.clone(),
        revision: receipt.1,
        kind: receipt.2,
        result: Err("保存线程异常，本次写入未完成，请重试".into()),
        warning: None,
    };
    // The UI releases in-flight flags only from a reply, so a panic inside a job
    // must still answer: otherwise the note stays on "保存中" and never saves again.
    match catch_unwind(AssertUnwindSafe(|| {
        process_job(repository, clock, job, receipt)
    })) {
        Ok(reply) => reply,
        Err(_) => unanswered,
    }
}

fn process_job(
    repository: &mut Repository,
    clock: &mut WriteClock,
    job: Job,
    (id, revision, kind): (String, u64, ReplyKind),
) -> Reply {
    match job {
        Job::Save {
            mut note,
            mut state,
            action,
            create,
            ..
        } => {
            clock.note(&mut note);
            if let Some(state) = &mut state {
                clock.window(state);
            }
            let result = save(repository, &note, state.as_ref(), action, create);
            reply(id, revision, kind, result)
        }
        Job::CloseEmpty { mut state, .. } => {
            clock.window(&mut state);
            let result = repository.close_empty(&id, &state).map(|()| None);
            reply(id, revision, kind, result)
        }
        Job::CloseWindow { mut state } => {
            clock.window(&mut state);
            reply(
                id,
                revision,
                kind,
                Ok(window_warning(repository.close_window(&state))),
            )
        }
        Job::SaveWindow { mut state } => {
            clock.window(&mut state);
            reply(
                id,
                revision,
                kind,
                repository.save_window(&state).map(|()| None),
            )
        }
        Job::Delete { mut state, .. } => {
            if let Some(state) = &mut state {
                clock.window(state);
            }
            let result = repository
                .soft_delete(&id, None, state.as_ref())
                .map(|()| None);
            reply(id, revision, kind, result)
        }
        Job::Restore { .. } => {
            let result = repository.restore(&id).map(|()| None);
            reply(id, revision, kind, result)
        }
        Job::Purge { .. } => {
            let result = repository.purge_note(&id);
            if result.is_ok() {
                clock.notes.remove(&id);
                clock.windows.remove(&format!("note:{id}"));
            }
            reply(id, revision, kind, result.map(|()| None))
        }
    }
}

fn save(
    repository: &mut Repository,
    note: &Note,
    state: Option<&WindowState>,
    action: SaveAction,
    create: bool,
) -> Result<Option<String>> {
    if let Some(state) = state {
        ensure!(
            state.note_id.as_deref() == Some(note.id.as_str()),
            "save window belongs to another note"
        );
        state.validate()?;
    }
    match action {
        SaveAction::Save => {
            if create {
                repository.save_draft(note)?;
            } else {
                repository.save_note(note)?;
            }
            Ok(state.and_then(|state| window_warning(repository.save_window(state))))
        }
        SaveAction::Close => {
            let state = state.context("closing a note requires its final window state")?;
            // Content must succeed before accepting a close. Window state is noncritical:
            // report its failure without preventing closure after the content is durable.
            if create {
                repository.save_draft(note)?;
            } else {
                repository.save_note(note)?;
            }
            Ok(window_warning(repository.close_window(state)))
        }
        SaveAction::Delete => {
            if create {
                repository.delete_draft(note, state)?;
            } else {
                repository.soft_delete(&note.id, Some(note), state)?;
            }
            Ok(None)
        }
    }
}

fn window_warning(result: Result<()>) -> Option<String> {
    result
        .err()
        .map(|error| format!("window state was not saved: {error:#}"))
}

fn reply(id: String, revision: u64, kind: ReplyKind, result: Result<Option<String>>) -> Reply {
    let (result, warning) = match result {
        Ok(warning) => (Ok(()), warning),
        Err(error) => (Err(format!("{error:#}")), None),
    };
    Reply {
        id,
        revision,
        kind,
        result,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Document;

    #[test]
    fn first_draft_commands_validate_before_insert_and_roll_back_failed_delete() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.db");
        let mut repository = Repository::open(&path)?;
        let mut note = Note {
            id: "saved".to_owned(),
            document: Document::from_json(
                r#"{"format_version":1,"blocks":[{"type":"paragraph","text":"draft content"}]}"#,
            )?,
            theme_id: "white".to_owned(),
            created_at: 1000,
            updated_at: 1000,
            deleted_at: None,
        };
        for (id, action) in [("saved", SaveAction::Save), ("closed", SaveAction::Close)] {
            note.id = id.to_owned();
            assert!(save(&mut repository, &note, None, SaveAction::Close, true).is_err());
            assert!(!repository.note_exists(id)?);
            let mut state = WindowState::for_note(id, 360.0, 420.0);
            state.normal_width = 0.0;
            assert!(save(&mut repository, &note, Some(&state), action, true).is_err());
            assert!(!repository.note_exists(id)?);
            state.normal_width = 360.0;
            assert!(save(&mut repository, &note, Some(&state), action, true)?.is_none());
            assert!(repository.note_exists(id)?);
        }
        assert_eq!(
            repository.startup_note()?.map(|note| note.id),
            Some("closed".to_owned())
        );

        note.id = "deleted".to_owned();
        let state = WindowState::for_note(&note.id, 360.0, 420.0);
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TRIGGER fail_draft_window BEFORE INSERT ON window_states
             WHEN NEW.note_id = 'deleted'
             BEGIN SELECT RAISE(ABORT, 'injected window write failure'); END;",
        )?;
        assert!(
            save(
                &mut repository,
                &note,
                Some(&state),
                SaveAction::Delete,
                true
            )
            .is_err()
        );
        assert!(!repository.note_exists(&note.id)?);
        assert!(repository.list_notes(true)?.is_empty());
        assert!(repository.window_state(&state.id)?.is_none());
        assert_eq!(
            repository.startup_note()?.map(|note| note.id),
            Some("closed".to_owned())
        );
        connection.execute_batch("DROP TRIGGER fail_draft_window;")?;
        assert!(
            save(
                &mut repository,
                &note,
                Some(&state),
                SaveAction::Delete,
                true
            )?
            .is_none()
        );
        let deleted = repository.list_notes(true)?;
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].id, note.id);
        assert_eq!(deleted[0].plain_text, "draft content");
        assert!(save(&mut repository, &note, None, SaveAction::Save, true).is_err());
        assert!(save(&mut repository, &note, None, SaveAction::Save, false).is_err());
        Ok(())
    }
}
