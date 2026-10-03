use super::*;

#[test]
fn clearing_a_saved_note_preserves_identity_while_draft_close_adds_no_rows() -> Result<()> {
    let mut repository = Repository::open(Path::new(":memory:"))?;
    let mut note = repository.create_note("blue")?;
    note.document = Document::from_json(
        r#"{"format_version":1,"blocks":[{"type":"paragraph","text":"content to clear"}]}"#,
    )?;
    note.updated_at = note.updated_at.saturating_add(1);
    repository.save_note(&note)?;
    // A formatting-only edit must survive persistence even when plain text is unchanged.
    note.document = Document::from_json(
        r#"{"format_version":1,"blocks":[{"type":"paragraph","text":"content to clear","bold":[{"start":0,"end":7}]}]}"#,
    )?;
    note.updated_at = note.updated_at.saturating_add(1);
    repository.save_note(&note)?;
    let styled = repository.note(&note.id)?.context("styled note missing")?;
    assert_eq!(styled.document.map_err(anyhow::Error::msg)?, note.document);
    assert_eq!(styled.updated_at, note.updated_at);
    note.document = Document::default();
    note.updated_at = note.updated_at.saturating_add(1);
    repository.save_note(&note)?;
    let state = WindowState::for_note(&note.id, 360.0, 420.0);
    repository.close_window(&state)?;
    let restored = repository
        .startup_note()?
        .context("empty saved note missing")?;
    assert_eq!(restored.id, note.id);
    assert_eq!(
        restored.document.map_err(anyhow::Error::msg)?,
        note.document
    );
    assert!(restored.plain_text.is_empty());
    assert!(restored.deleted_at.is_none());
    assert!(repository.list_notes(true)?.is_empty());
    let mut empty_draft = note.clone();
    empty_draft.id = "must-not-be-created".to_owned();
    assert!(repository.save_draft(&empty_draft).is_err());

    repository.close_draft()?;
    repository.close_draft()?;
    assert!(repository.startup_note()?.is_none());
    assert_eq!(repository.list_notes(false)?.len(), 1);
    assert!(repository.note_exists(&note.id)?);
    assert_eq!(
        repository.window_state(&state.id)?.map(|state| state.id),
        Some(state.id)
    );
    let counts: (i64, i64) = repository.connection.query_row(
        "SELECT (SELECT COUNT(*) FROM window_states), (SELECT COUNT(*) FROM app_state)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(counts, (1, 0));
    Ok(())
}

#[test]
fn deletion_is_atomic_and_late_autosave_cannot_resurrect_note() -> Result<()> {
    let mut repository = Repository::open(Path::new(":memory:"))?;
    let note = repository.create_note("yellow")?;
    let state = WindowState::for_note(&note.id, 360.0, 420.0);
    repository.close_window(&state)?;
    assert_eq!(
        repository.startup_note()?.map(|note| note.id),
        Some(note.id.clone())
    );
    repository.soft_delete(&note.id, Some(&note), Some(&state))?;
    assert!(repository.save_note(&note).is_err());
    assert!(repository.startup_note()?.is_none());
    assert_eq!(repository.list_notes(true)?.len(), 1);
    repository.restore(&note.id)?;
    assert_eq!(
        repository.window_state(&state.id)?.map(|state| state.id),
        Some(state.id)
    );
    assert_eq!(repository.list_notes(false)?.len(), 1);
    repository.close_window(&WindowState::for_aux("notes_list", 600.0, 500.0))?;
    assert!(repository.startup_note()?.is_none());

    let mut draft = note.clone();
    draft.id = "unpersisted-draft".to_owned();
    let draft_state = WindowState::for_note(&draft.id, 360.0, 420.0);
    assert!(repository.save_draft(&draft).is_err());
    repository.close_draft()?;
    assert!(repository.startup_note()?.is_none());
    assert!(repository.window_state(&draft_state.id)?.is_none());
    draft.document = Document::from_json(
        r#"{"format_version":1,"blocks":[{"type":"paragraph","text":"durable content"}]}"#,
    )?;
    repository.save_draft(&draft)?;
    assert!(repository.save_draft(&draft).is_err());
    repository.soft_delete(&draft.id, Some(&draft), Some(&draft_state))?;
    assert!(repository.save_draft(&draft).is_err());
    let deleted = repository.list_notes(true)?;
    assert_eq!(
        deleted
            .iter()
            .find(|note| note.id == draft.id)
            .map(|note| note.plain_text.as_str()),
        Some("durable content")
    );
    Ok(())
}

#[test]
fn unreadable_document_is_preserved_and_does_not_block_other_notes() -> Result<()> {
    let mut repository = Repository::open(Path::new(":memory:"))?;
    let damaged = repository.create_note("blue")?;
    repository.create_note("yellow")?;
    let raw = r#"{"format_version":999,"blocks":[]}"#;
    repository.connection.execute(
        "UPDATE notes SET document_json = ?2 WHERE id = ?1",
        params![damaged.id, raw],
    )?;
    assert_eq!(repository.list_notes(false)?.len(), 2);
    assert!(
        repository
            .note(&damaged.id)?
            .context("note missing")?
            .document
            .is_err()
    );
    assert!(repository.save_note(&damaged).is_err());
    repository.soft_delete(&damaged.id, None, None)?;
    repository.restore(&damaged.id)?;
    let preserved: String = repository.connection.query_row(
        "SELECT document_json FROM notes WHERE id = ?1",
        [damaged.id],
        |row| row.get(0),
    )?;
    assert_eq!(preserved, raw);
    Ok(())
}

#[test]
fn draft_migration_drops_only_the_history_rows_the_new_checks_reject() -> Result<()> {
    let cases: [(&str, Option<&str>, usize); 4] = [
        ("note", None, 0),
        ("notes_list", Some("stray"), 0),
        ("settings", None, 1),
        ("note", Some("missing"), 1),
    ];
    for (window_type, note_id, retained) in cases {
        let file = tempfile::NamedTempFile::new().context("temp file")?;
        let path = file.path().to_path_buf();
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch(INITIAL_SCHEMA)?;
            connection.execute_batch(
                "DROP TABLE app_state;
                 CREATE TABLE app_state (
                     id INTEGER PRIMARY KEY, last_closed_window_type TEXT NOT NULL,
                     last_closed_note_id TEXT, updated_at INTEGER NOT NULL
                 );",
            )?;
            let stored = note_id
                .map(|id| format!("'{id}'"))
                .unwrap_or_else(|| "NULL".to_owned());
            connection.execute_batch(&format!(
                "INSERT INTO app_state (id, last_closed_window_type, last_closed_note_id, updated_at)
                 VALUES (1, '{window_type}', {stored}, 1000);"
            ))?;
            connection.pragma_update(None, "user_version", 1)?;
        }
        let repository = Repository::open(&path)?;
        assert!(
            repository.startup_note()?.is_none(),
            "{window_type} must not open a note window"
        );
        drop(repository);
        let connection = Connection::open(&path)?;
        let kept: usize = connection.query_row("SELECT COUNT(*) FROM app_state", [], |row| {
            row.get::<_, i64>(0).map(|count| count as usize)
        })?;
        assert_eq!(kept, retained, "{window_type} keeps only usable history");
        let rejected: usize = connection.query_row(
            "SELECT COUNT(*) FROM app_state
             WHERE NOT ((last_closed_window_type = 'note' AND last_closed_note_id IS NOT NULL)
                 OR (last_closed_window_type IN ('notes_list', 'settings') AND last_closed_note_id IS NULL))",
            [],
            |row| row.get::<_, i64>(0).map(|count| count as usize),
        )?;
        assert_eq!(rejected, 0, "{window_type} leaves no row the CHECK rejects");
    }
    Ok(())
}

#[test]
fn version_zero_adoption_preserves_notes_and_allows_draft_close() -> Result<()> {
    let directory = tempfile::tempdir().context("temp directory")?;
    let path = directory.path().join("notes.db");
    let raw = r#"{"format_version":1,"blocks":[{"type":"paragraph","text":"preserved content"}]}"#;
    {
        let connection = Connection::open(&path)?;
        connection.execute_batch(INITIAL_SCHEMA)?;
        connection.execute_batch(
            "DROP TABLE app_state;
             CREATE TABLE app_state (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 last_closed_window_type TEXT NOT NULL,
                 last_closed_note_id TEXT REFERENCES notes(id),
                 updated_at INTEGER NOT NULL
             );",
        )?;
        connection.execute(
            "INSERT INTO notes (id, document_json, plain_text, theme_id, created_at, updated_at)
             VALUES ('saved', ?1, 'preserved content', 'blue', 1000, 1000)",
            [raw],
        )?;
        connection.execute_batch("INSERT INTO app_state VALUES (1, 'note', 'saved', 1000);")?;
    }

    let mut repository = Repository::open(&path)?;
    assert_eq!(
        repository.startup_note()?.map(|note| note.id),
        Some("saved".to_owned())
    );
    let preserved: String = repository.connection.query_row(
        "SELECT document_json FROM notes WHERE id = 'saved'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(preserved, raw);
    let version: i64 = repository
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);

    let draft = WindowState::for_note("unpersisted-draft", 360.0, 420.0);
    repository.close_draft()?;
    assert!(repository.window_state(&draft.id)?.is_none());
    assert!(repository.startup_note()?.is_none());
    assert!(repository.note_exists("saved")?);
    Ok(())
}

#[test]
fn a_version_zero_database_is_refused_before_it_is_touched() -> Result<()> {
    let foreign = [("notes", "id TEXT PRIMARY KEY, title TEXT")];
    let mismatched = [
        ("notes", "id TEXT PRIMARY KEY, document_json TEXT NOT NULL"),
        (
            "window_states",
            "id TEXT PRIMARY KEY, normal_width REAL NOT NULL",
        ),
        (
            "app_state",
            "id INTEGER PRIMARY KEY, last_closed_window_type TEXT NOT NULL",
        ),
    ];
    for (tables, expected) in [(&foreign[..], "notes"), (&mismatched[..], "plain_text")] {
        let file = tempfile::NamedTempFile::new().context("temp file")?;
        let path = file.path().to_path_buf();
        for (name, body) in tables {
            let connection = Connection::open(&path)?;
            connection
                .execute_batch(&format!("CREATE TABLE {name} ({body});"))
                .context("foreign table")?;
        }
        let error = match Repository::open(&path) {
            Ok(_) => panic!("must refuse an unknown layout"),
            Err(error) => error,
        };
        let message = format!("{error:#}");
        assert!(message.contains(expected), "{message}");
        let connection = Connection::open(&path)?;
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let names: Vec<String> = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut before: Vec<String> = tables.iter().map(|(name, _)| name.to_string()).collect();
        before.sort_unstable();
        assert_eq!(names, before, "no table may be created or dropped");
    }
    Ok(())
}
