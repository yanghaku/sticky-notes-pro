//! Note queries and atomic content, deletion, and restoration operations.

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::{fs, path::Path, time::Duration};
#[cfg(test)]
use uuid::Uuid;

use super::{
    WindowState, now_millis,
    paths::parent_directory,
    schema::{DRAFT_STATE_MIGRATION, INITIAL_SCHEMA, OWNED_TABLES, SCHEMA_VERSION},
    windows::{WINDOW_COLUMNS, close_window_in, read_window, record_close_in, save_window_in},
};
use crate::model::{Document, Note};

const NOTE_COLUMNS: &str =
    "id, document_json, theme_id, plain_text, created_at, updated_at, deleted_at";

/// A malformed document is an isolated read error, rather than an empty editable note.
#[derive(Debug, Clone)]
pub struct StoredNote {
    pub id: String,
    pub document: std::result::Result<Document, String>,
    pub theme_id: String,
    pub plain_text: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

/// Writes take the write lock up front. A deferred transaction that starts with
/// a read cannot wait when it later asks for the write lock — SQLite fails it
/// with SQLITE_BUSY immediately to avoid deadlock, and busy_timeout never runs —
/// which matters because the worker and the UI own separate connections.
fn write_transaction(connection: &mut Connection) -> Result<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("cannot begin database write")
}

/// Version 0 is both "empty file" and "database from before schema versioning".
/// Only our own, recognisably shaped tables may be adopted; every other layout is
/// refused before a statement runs, so foreign data is never modified.
/// Returns whether existing tables need to be adopted and migrated.
fn refuse_foreign_database(transaction: &Transaction<'_>, path: &Path) -> Result<bool> {
    let mut statement = transaction
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .context("cannot inspect database tables")?;
    let mut found = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("cannot inspect database tables")?;
    if found.is_empty() {
        return Ok(false);
    }
    found.sort_unstable();
    let mut owned: Vec<&str> = OWNED_TABLES.iter().map(|(name, _)| *name).collect();
    owned.sort_unstable();
    let foreign: Vec<&str> = found
        .iter()
        .map(String::as_str)
        .filter(|name| !owned.contains(name))
        .collect();
    let missing: Vec<&str> = owned
        .iter()
        .copied()
        .filter(|name| !found.iter().any(|it| it.as_str() == *name))
        .collect();
    ensure!(
        foreign.is_empty() && missing.is_empty(),
        "{} 不是本应用可识别的数据库：现有数据表 {found:?}（多余 {foreign:?}，缺少 {missing:?}）。未做任何修改，请先将该文件移出数据目录。",
        path.display()
    );
    for (table, required) in OWNED_TABLES {
        let columns = table_columns(transaction, table)?;
        let absent: Vec<&str> = required
            .iter()
            .copied()
            .filter(|column| !columns.iter().any(|name| name.as_str() == *column))
            .collect();
        ensure!(
            absent.is_empty(),
            "{} 中的表 {table} 缺少本应用需要的字段 {absent:?}。未做任何修改，请先将该文件移出数据目录。",
            path.display()
        );
    }
    Ok(true)
}

fn table_columns(transaction: &Transaction<'_>, table: &str) -> Result<Vec<String>> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("cannot inspect table {table}"))
}

/// Owned by one thread. rusqlite connections are Send but not Sync.
pub struct Repository {
    connection: Connection,
}

impl Repository {
    pub fn open(path: &Path) -> Result<Self> {
        fs::create_dir_all(parent_directory(path)).context("cannot create database directory")?;
        let mut connection = Connection::open(path).context("cannot open notes database")?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .context("cannot configure database timeout")?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .context("cannot enable database foreign keys")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "database schema version {version} is newer than supported version {SCHEMA_VERSION}"
        );
        ensure!(version >= 0, "invalid database schema version");
        if version == 0 {
            let transaction = write_transaction(&mut connection)?;
            let adopting = refuse_foreign_database(&transaction, path)?;
            transaction
                .execute_batch(INITIAL_SCHEMA)
                .context("cannot migrate database")?;
            if adopting {
                // IF NOT EXISTS preserves old definitions, including the notes FK
                // that prevents recording the close of an unpersisted draft.
                transaction
                    .execute_batch(DRAFT_STATE_MIGRATION)
                    .context("cannot migrate adopted draft close records")?;
            }
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction
                .commit()
                .context("cannot commit database migration")?;
        } else if version == 1 {
            let transaction = write_transaction(&mut connection)?;
            transaction
                .execute_batch(DRAFT_STATE_MIGRATION)
                .context("cannot migrate draft close records")?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction
                .commit()
                .context("cannot commit draft support migration")?;
        }
        // Early development databases used an updated_at index; add the stable-list
        // index without discarding either existing content or the older harmless index.
        connection
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS notes_created ON notes (created_at DESC, id)",
            )
            .context("cannot prepare stable note list index")?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .context("cannot enable write-ahead logging")?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .context("cannot configure database durability")?;
        Ok(Self { connection })
    }

    pub fn startup_note(&self) -> Result<Option<StoredNote>> {
        let sql = format!(
            "SELECT {NOTE_COLUMNS} FROM notes WHERE deleted_at IS NULL AND id = (SELECT last_closed_note_id FROM app_state WHERE id = 1 AND last_closed_window_type = 'note')"
        );
        self.connection
            .query_row(&sql, [], stored_note)
            .optional()
            .context("cannot read startup note")
    }

    pub fn note(&self, id: &str) -> Result<Option<StoredNote>> {
        let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1 AND deleted_at IS NULL");
        self.connection
            .query_row(&sql, [id], stored_note)
            .optional()
            .context("cannot read note")
    }

    /// Check persistence without loading or parsing the note's content again.
    pub fn note_exists(&self, id: &str) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?1 AND deleted_at IS NULL)",
                [id],
                |row| row.get(0),
            )
            .context("cannot check note persistence")
    }

    #[cfg(test)]
    fn create_note(&mut self, theme_id: &str) -> Result<Note> {
        let now = now_millis();
        let note = Note {
            id: Uuid::new_v4().to_string(),
            document: Document::default(),
            theme_id: theme_id.to_owned(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let json = document_json(&note.document)?;
        self.connection.execute(
            "INSERT INTO notes (id, document_json, plain_text, theme_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![note.id, json, note.document.plain_text(), note.theme_id, note.created_at, note.updated_at],
        ).context("cannot create note")?;
        Ok(note)
    }

    /// Persist a nonempty draft once. A colliding live or deleted identity is never replaced.
    pub fn save_draft(&mut self, note: &Note) -> Result<()> {
        insert_draft_in(&self.connection, note, None)
    }

    /// Create the final draft snapshot already soft-deleted, with any close record
    /// in the same transaction. Failed deletion must not leave a newly persisted row.
    pub fn delete_draft(&mut self, note: &Note, state: Option<&WindowState>) -> Result<()> {
        if let Some(state) = state {
            ensure!(
                state.note_id.as_deref() == Some(note.id.as_str()),
                "deletion window belongs to another note"
            );
            state.validate()?;
        }
        let transaction = write_transaction(&mut self.connection)?;
        insert_draft_in(&transaction, note, Some(now_millis()))?;
        if let Some(state) = state {
            close_window_in(&transaction, state)?;
        }
        transaction.commit().context("cannot commit draft deletion")
    }

    #[cfg(test)]
    pub fn list_notes(&self, deleted: bool) -> Result<Vec<StoredNote>> {
        self.map_notes(deleted, std::convert::identity)
    }

    /// Project one database row at a time, so list previews never retain every
    /// full document at once. Results keep the database's stable view ordering.
    pub fn map_notes<T>(
        &self,
        deleted: bool,
        mut project: impl FnMut(StoredNote) -> T,
    ) -> Result<Vec<T>> {
        let (filter, order) = if deleted {
            ("deleted_at IS NOT NULL", "deleted_at DESC, id ASC")
        } else {
            ("deleted_at IS NULL", "created_at DESC, id ASC")
        };
        let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE {filter} ORDER BY {order}");
        let mut statement = self
            .connection
            .prepare(&sql)
            .context("cannot query note list")?;
        statement
            .query_map([], stored_note)?
            .map(|row| row.map(&mut project))
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("cannot read note list")
    }

    /// Update existing live notes only. A late autosave can never resurrect a deleted note.
    pub fn save_note(&mut self, note: &Note) -> Result<()> {
        ensure!(note.deleted_at.is_none(), "cannot edit a deleted note");
        let transaction = write_transaction(&mut self.connection)?;
        save_note_in(&transaction, note)?;
        transaction.commit().context("cannot commit note save")
    }

    pub fn save_window(&mut self, state: &WindowState) -> Result<()> {
        let transaction = write_transaction(&mut self.connection)?;
        save_window_in(&transaction, state)?;
        transaction.commit().context("cannot commit window state")
    }

    pub fn close_window(&mut self, state: &WindowState) -> Result<()> {
        let transaction = write_transaction(&mut self.connection)?;
        close_window_in(&transaction, state)?;
        transaction
            .commit()
            .context("cannot commit closed window state")
    }

    /// Unpersisted drafts only leave a close record. Clearing a saved note soft-deletes
    /// its last durable content, preserving that content for the deleted view and recovery.
    pub fn close_empty(&mut self, id: &str, state: &WindowState) -> Result<()> {
        state.validate()?;
        ensure!(
            state.note_id.as_deref() == Some(id),
            "empty window belongs to another note"
        );
        let transaction = write_transaction(&mut self.connection)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )?;
        if exists {
            close_window_in(&transaction, state)?;
            let changed = transaction.execute(
                "UPDATE notes SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_millis()],
            )?;
            ensure!(changed == 1, "note is already deleted");
        } else {
            record_close_in(&transaction, state)?;
        }
        transaction
            .commit()
            .context("cannot commit empty note close")
    }

    pub fn window_state(&self, id: &str) -> Result<Option<WindowState>> {
        let sql = format!("SELECT {WINDOW_COLUMNS} FROM window_states WHERE id = ?1");
        let state = self
            .connection
            .query_row(&sql, [id], read_window)
            .optional()
            .context("cannot read window state")?;
        if let Some(state) = &state {
            state.validate()?;
        }
        Ok(state)
    }

    pub fn recent_note_window(&self) -> Result<Option<WindowState>> {
        let columns = WINDOW_COLUMNS
            .split(", ")
            .map(|column| format!("w.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {columns} FROM window_states w JOIN notes n ON n.id = w.note_id WHERE n.deleted_at IS NULL ORDER BY w.last_used_at DESC, w.updated_at DESC, w.id ASC LIMIT 1"
        );
        let state = self
            .connection
            .query_row(&sql, [], read_window)
            .optional()
            .context("cannot read recent note window")?;
        if let Some(state) = &state {
            state.validate()?;
        }
        Ok(state)
    }

    /// Save the final editor snapshot, close record, and soft-delete marker together.
    /// With no readable snapshot, content bytes and the projection remain untouched.
    pub fn soft_delete(
        &mut self,
        id: &str,
        latest: Option<&Note>,
        state: Option<&WindowState>,
    ) -> Result<()> {
        let transaction = write_transaction(&mut self.connection)?;
        if let Some(note) = latest {
            ensure!(
                note.id == id && note.deleted_at.is_none(),
                "invalid deletion snapshot"
            );
            save_note_in(&transaction, note)?;
        }
        if let Some(state) = state {
            ensure!(
                state.note_id.as_deref() == Some(id),
                "deletion window belongs to another note"
            );
            close_window_in(&transaction, state)?;
        }
        let changed = transaction.execute(
            "UPDATE notes SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now_millis()],
        )?;
        ensure!(changed == 1, "note is missing or already deleted");
        transaction.commit().context("cannot commit note deletion")
    }

    /// Restore metadata only; malformed documents remain visibly unreadable after restoration.
    pub fn restore(&mut self, id: &str) -> Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE notes SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .context("cannot restore note")?;
        ensure!(changed == 1, "note is missing or already restored");
        Ok(())
    }

    /// Permanently remove one already-deleted note and its dependent application state.
    ///
    /// Returns an error for a live or missing note. All cleanup rolls back if any step
    /// fails, including if the final conditional delete does not affect exactly one row.
    pub fn purge_note(&mut self, id: &str) -> Result<()> {
        ensure!(!id.is_empty(), "note identity cannot be empty");
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("cannot begin permanent note deletion")?;
        let deleted: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?1 AND deleted_at IS NOT NULL)",
                [id],
                |row| row.get(0),
            )
            .context("cannot verify deleted note identity")?;
        ensure!(deleted, "note is missing or has not been soft-deleted");
        // Window state has a notes FK without cascading deletion. Remove only this
        // note's row; neither unrelated note state nor auxiliary windows are touched.
        transaction
            .execute("DELETE FROM window_states WHERE note_id = ?1", [id])
            .context("cannot remove deleted note window state")?;
        // A note close record requires a nonnull note id, so clearing the entire
        // matching history row restores the normal no-history startup behavior.
        transaction
            .execute("DELETE FROM app_state WHERE last_closed_note_id = ?1", [id])
            .context("cannot clear permanently deleted note close record")?;
        let changed = transaction
            .execute(
                "DELETE FROM notes WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .context("cannot permanently delete note")?;
        ensure!(
            changed == 1,
            "permanent deletion requires exactly one deleted note"
        );
        transaction
            .commit()
            .context("cannot commit permanent note deletion")
    }
}

fn document_json(document: &Document) -> Result<String> {
    document.validate()?;
    serde_json::to_string(document).context("cannot serialize note document")
}

fn insert_draft_in(connection: &Connection, note: &Note, deleted_at: Option<i64>) -> Result<()> {
    ensure!(note.deleted_at.is_none(), "cannot create a deleted draft");
    let json = document_json(&note.document)?;
    let projection = note.document.plain_text();
    ensure!(
        !projection.trim().is_empty(),
        "empty draft notes are not persisted"
    );
    connection.execute(
        "INSERT INTO notes (id, document_json, plain_text, theme_id, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![note.id, json, projection, note.theme_id, note.created_at, note.updated_at, deleted_at],
    ).context("cannot persist draft; an existing note identity cannot be overwritten")?;
    Ok(())
}

fn stored_note(row: &Row<'_>) -> rusqlite::Result<StoredNote> {
    let json: String = row.get(1)?;
    Ok(StoredNote {
        id: row.get(0)?,
        document: Document::from_json(&json).map_err(|error| format!("{error:#}")),
        theme_id: row.get(2)?,
        plain_text: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
    })
}

fn save_note_in(transaction: &Transaction<'_>, note: &Note) -> Result<()> {
    ensure!(
        note.deleted_at.is_none(),
        "cannot save deleted note content"
    );
    let json = document_json(&note.document)?;
    // Recheck the stored document too: callers must not replace unknown content formats
    // with a newly-created editable default, even if they bypass the normal load path.
    let current: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT document_json, theme_id, plain_text FROM notes WHERE id = ?1 AND deleted_at IS NULL",
            [&note.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (current_json, current_theme, current_projection) =
        current.context("note is missing or deleted")?;
    let current_document = Document::from_json(&current_json)
        .context("refusing to overwrite unreadable note document")?;
    let projection = note.document.plain_text();
    if current_document == note.document && current_theme == note.theme_id {
        // Closing or moving a window must not advance content timestamps or reorder cards.
        if current_projection != projection {
            transaction
                .execute(
                    "UPDATE notes SET plain_text = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    params![note.id, projection],
                )
                .context("cannot repair note text projection")?;
        }
        return Ok(());
    }
    let changed = transaction.execute(
        "UPDATE notes SET document_json = ?2, plain_text = ?3, theme_id = ?4, updated_at = ?5 WHERE id = ?1 AND deleted_at IS NULL AND updated_at <= ?5",
        params![note.id, json, projection, note.theme_id, note.updated_at],
    ).context("cannot save note")?;
    ensure!(changed == 1, "refusing stale note save");
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
