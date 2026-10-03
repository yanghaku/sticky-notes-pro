//! SQLite schema declarations; versions are independent of Document formats.

pub(super) const SCHEMA_VERSION: i64 = 2;

pub(super) const INITIAL_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY, document_json TEXT NOT NULL, plain_text TEXT NOT NULL,
    theme_id TEXT NOT NULL DEFAULT 'white', created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL, deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS notes_created ON notes (created_at DESC, id);
CREATE INDEX IF NOT EXISTS notes_deleted ON notes (deleted_at DESC, id);
CREATE TABLE IF NOT EXISTS window_states (
    id TEXT PRIMARY KEY,
    window_type TEXT NOT NULL CHECK (window_type IN ('note', 'notes_list', 'settings')),
    note_id TEXT UNIQUE REFERENCES notes(id),
    normal_x REAL, normal_y REAL,
    normal_width REAL NOT NULL CHECK (normal_width > 0),
    normal_height REAL NOT NULL CHECK (normal_height > 0),
    monitor_id TEXT, scale_factor REAL CHECK (scale_factor > 0),
    maximized INTEGER NOT NULL DEFAULT 0 CHECK (maximized IN (0, 1)),
    always_on_top INTEGER NOT NULL DEFAULT 0 CHECK (always_on_top IN (0, 1)),
    updated_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL,
    CHECK ((normal_x IS NULL) = (normal_y IS NULL)),
    CHECK ((window_type = 'note' AND note_id IS NOT NULL AND id = 'note:' || note_id)
        OR (window_type IN ('notes_list', 'settings') AND note_id IS NULL AND id = window_type))
);
CREATE TABLE IF NOT EXISTS app_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_closed_window_type TEXT NOT NULL CHECK (last_closed_window_type IN ('note', 'notes_list', 'settings')),
    last_closed_note_id TEXT, updated_at INTEGER NOT NULL,
    CHECK ((last_closed_window_type = 'note' AND last_closed_note_id IS NOT NULL)
        OR (last_closed_window_type IN ('notes_list', 'settings') AND last_closed_note_id IS NULL))
);";

/// Tables the version 0 step may adopt instead of refusing, with the columns they
/// must expose. Anything else in the file means it is not this application's data.
pub(super) const OWNED_TABLES: [(&str, &[&str]); 3] = [
    (
        "notes",
        &[
            "id",
            "document_json",
            "plain_text",
            "theme_id",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
    ),
    (
        "window_states",
        &[
            "id",
            "window_type",
            "note_id",
            "normal_x",
            "normal_y",
            "normal_width",
            "normal_height",
            "monitor_id",
            "scale_factor",
            "maximized",
            "always_on_top",
            "updated_at",
            "last_used_at",
        ],
    ),
    (
        "app_state",
        &[
            "id",
            "last_closed_window_type",
            "last_closed_note_id",
            "updated_at",
        ],
    ),
];

// A closed draft has a stable id without a notes row. Application history may retain
// that id, while note windows continue requiring a durable notes foreign-key target.
// The copy below also drops rows the new CHECK would reject, so one bad history row
// cannot abort the whole migration.
pub(super) const DRAFT_STATE_MIGRATION: &str = "
ALTER TABLE app_state RENAME TO app_state_v1;
CREATE TABLE app_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_closed_window_type TEXT NOT NULL CHECK (last_closed_window_type IN ('note', 'notes_list', 'settings')),
    last_closed_note_id TEXT, updated_at INTEGER NOT NULL,
    CHECK ((last_closed_window_type = 'note' AND last_closed_note_id IS NOT NULL)
        OR (last_closed_window_type IN ('notes_list', 'settings') AND last_closed_note_id IS NULL))
);
DELETE FROM app_state_v1
    WHERE id <> 1
        OR last_closed_window_type NOT IN ('note', 'notes_list', 'settings')
        OR (last_closed_window_type = 'note' AND last_closed_note_id IS NULL)
        OR (last_closed_window_type <> 'note' AND last_closed_note_id IS NOT NULL);
INSERT INTO app_state (id, last_closed_window_type, last_closed_note_id, updated_at)
SELECT id, last_closed_window_type,
       CASE WHEN last_closed_window_type = 'note' THEN last_closed_note_id END,
       updated_at FROM app_state_v1;
DROP TABLE app_state_v1;
";
