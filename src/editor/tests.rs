use super::*;
use crate::model::ListItem;

#[test]
fn automatic_list_conversion_undo_restores_literal_prefix() {
    let mut editor = Editor::new(Document::default());
    editor.insert_text("1.", true);
    editor.insert_text(" ", true);
    assert_eq!(editor.rows()[0].marker, "1. ");
    assert!(editor.undo());
    assert_eq!(editor.document().display_text(), "1. ");
    assert_eq!(editor.selection().1.offset, 3);
    assert!(editor.redo());
    assert_eq!(editor.rows()[0].marker, "1. ");

    let mut ime = Editor::new(Document::default());
    ime.commit_text("1. ");
    assert_eq!(ime.rows()[0].marker, "1. ");
    ime.undo();
    assert_eq!(ime.rows()[0].text, "1. ");
    ime.undo();
    assert_eq!(ime.document(), &Document::default());
}

#[test]
fn delete_whole_item_renumbers_and_undo_restores_selection() {
    let mut editor = Editor::new(Document {
        format_version: 1,
        blocks: vec![Block::OrderedList {
            items: ["A", "B", "C", "D", "E", "F"]
                .into_iter()
                .map(|text| ListItem { text: text.into() })
                .collect(),
            continuation: false,
        }],
    });
    let selection = (
        Position { row: 1, offset: 0 },
        Position { row: 2, offset: 0 },
    );
    editor.select(selection.0, selection.1);
    assert!(editor.backspace());
    assert_eq!(
        editor.document().display_text(),
        "1. A\n2. C\n3. D\n4. E\n5. F"
    );
    editor.undo();
    assert_eq!(editor.selection(), selection);
    assert_eq!(
        editor.document().display_text(),
        "1. A\n2. B\n3. C\n4. D\n5. E\n6. F"
    );
    let mut continued = Editor::new(editor.document().clone());
    continued.select(
        Position { row: 1, offset: 0 },
        Position { row: 1, offset: 0 },
    );
    continued.backspace();
    assert_eq!(
        continued.document().display_text(),
        "1. A\nB\n2. C\n3. D\n4. E\n5. F"
    );
    continued.select(
        Position { row: 2, offset: 0 },
        Position { row: 2, offset: 0 },
    );
    continued.backspace();
    assert_eq!(
        continued.document().display_text(),
        "1. A\nB\nC\n2. D\n3. E\n4. F"
    );
    let json = serde_json::to_string(continued.document()).expect("serialize continuation");
    let reopened = Editor::new(Document::from_json(&json).expect("read continuation"));
    assert_eq!(
        reopened
            .rows()
            .iter()
            .map(|row| row.marker)
            .collect::<Vec<_>>(),
        ["1. ", "", "", "2. ", "3. ", "4. "]
    );
    continued.undo();
    assert_eq!(
        continued.document().display_text(),
        "1. A\nB\n2. C\n3. D\n4. E\n5. F"
    );
    assert_eq!(continued.selection().1, Position { row: 2, offset: 0 });
    continued.redo();
    continued.select_all();
    assert_eq!(continued.clipboard_text(), "1. A\nB\nC\n2. D\n3. E\n4. F");
    editor.select(
        Position { row: 2, offset: 0 },
        Position { row: 2, offset: 0 },
    );
    editor.backspace();
    assert_eq!(
        editor.document().display_text(),
        "1. A\n2. B\nC\n3. D\n4. E\n5. F"
    );
    editor.backspace();
    assert_eq!(
        editor.document().display_text(),
        "1. A\n2. BC\n3. D\n4. E\n5. F"
    );
    assert_eq!(editor.document().blocks.len(), 1);
    editor.select(
        Position { row: 1, offset: 2 },
        Position { row: 1, offset: 2 },
    );
    editor.delete_forward();
    assert_eq!(editor.document().display_text(), "1. A\n2. BCD\n3. E\n4. F");

    let mut mixed = Editor::new(Document {
        format_version: 1,
        blocks: vec![
            Block::OrderedList {
                items: ["A", "B"]
                    .into_iter()
                    .map(|text| ListItem { text: text.into() })
                    .collect(),
                continuation: false,
            },
            Block::Paragraph { text: "C".into() },
        ],
    });
    mixed.select(
        Position { row: 1, offset: 0 },
        Position { row: 2, offset: 0 },
    );
    mixed.delete_forward();
    assert_eq!(mixed.document().display_text(), "1. A\n2. C");
}

#[test]
fn exit_middle_item_preserves_ordered_continuation_and_paste_keeps_empty_items() {
    let mut editor = Editor::new(Document::default());
    editor.insert_text("1.", true);
    editor.insert_text(" ", true);
    editor.insert_text("A\n\nB", false);
    editor.select(
        Position { row: 1, offset: 0 },
        Position { row: 1, offset: 0 },
    );
    editor.enter();
    assert_eq!(editor.document().display_text(), "1. A\n\n2. B");
    assert_eq!(editor.document().blocks.len(), 3);
    editor.undo();
    assert_eq!(editor.document().display_text(), "1. A\n2. \n3. B");
    editor.redo();
    editor.backspace();
    assert_eq!(editor.document().display_text(), "1. A\n2. B");
    assert_eq!(editor.document().blocks.len(), 1);
    editor.select(
        Position { row: 1, offset: 0 },
        Position { row: 1, offset: 0 },
    );
    editor.backspace();
    assert_eq!(editor.document().display_text(), "1. A\nB");
    editor.backspace();
    assert_eq!(editor.document().display_text(), "1. AB");
}

#[test]
fn grapheme_deletion_and_cross_block_replacement_preserve_text() {
    let mut editor = Editor::new(Document::default());
    editor.insert_text("A👨‍👩‍👧‍👦e\u{301}\nLast", false);
    editor.select(
        Position {
            row: 0,
            offset: "A👨‍👩‍👧‍👦e\u{301}".len(),
        },
        Position {
            row: 0,
            offset: "A👨‍👩‍👧‍👦e\u{301}".len(),
        },
    );
    editor.backspace();
    assert_eq!(editor.rows()[0].text, "A👨‍👩‍👧‍👦");
    editor.backspace();
    assert_eq!(editor.rows()[0].text, "A");
    editor.select(
        Position { row: 0, offset: 1 },
        Position { row: 1, offset: 2 },
    );
    editor.insert_text("x\r\ny", false);
    assert_eq!(editor.document().display_text(), "Ax\nyst");
    editor.select_all();
    editor.delete_forward();
    assert_eq!(editor.document(), &Document::default());
}

#[test]
fn bullet_prefix_history_paste_exit_and_json_follow_list_rules() {
    for prefix in ["-", "*"] {
        let mut editor = Editor::new(Document::default());
        editor.insert_text(prefix, true);
        editor.insert_text(" ", true);
        assert_eq!(editor.rows()[0].marker, "• ");
        assert!(editor.document().is_empty());
        editor.undo();
        assert_eq!(editor.document().display_text(), format!("{prefix} "));
        editor.redo();
        editor.insert_text("Milk\nBread\n", false);
        assert_eq!(editor.document().display_text(), "• Milk\n• Bread\n• ");
        editor.select_all();
        assert_eq!(editor.clipboard_text(), "• Milk\n• Bread\n");
        editor.end(false);
        editor.enter();
        assert_eq!(editor.document().display_text(), "• Milk\n• Bread\n");
        let json = serde_json::to_string(editor.document()).expect("serialize bullet document");
        assert_eq!(
            Document::from_json(&json).expect("read bullet document"),
            *editor.document()
        );
        editor.select(
            Position { row: 1, offset: 0 },
            Position { row: 1, offset: 0 },
        );
        editor.backspace();
        assert_eq!(editor.document().display_text(), "• Milk\nBread\n");
        editor.backspace();
        assert_eq!(editor.document().display_text(), "• MilkBread\n");
        editor.select_all();
        editor.insert_text("- literal\n* literal", false);
        assert_eq!(editor.document().display_text(), "- literal\n* literal");
        assert!(editor.rows().iter().all(|row| row.marker.is_empty()));
    }
    let legacy = Document::from_json(
        r#"{"format_version":1,"blocks":[{"type":"ordered_list","items":[{"text":" \t"}]}]}"#,
    )
    .expect("legacy version 1 content");
    assert!(legacy.is_empty());
    assert_eq!(legacy.display_text(), "1.  \t");
}

#[test]
fn copied_list_markers_are_not_stored_when_pasting_into_a_list() {
    let mut editor = Editor::new(Document {
        format_version: 1,
        blocks: vec![Block::OrderedList {
            items: ["A", "B", "C"]
                .into_iter()
                .map(|text| ListItem { text: text.into() })
                .collect(),
            continuation: false,
        }],
    });
    editor.select(
        Position { row: 1, offset: 0 },
        Position { row: 2, offset: 1 },
    );
    assert_eq!(editor.clipboard_text(), "2. B\n3. C");
    let copied = editor.clipboard_text();
    editor.select(
        Position { row: 2, offset: 1 },
        Position { row: 2, offset: 1 },
    );
    assert!(editor.enter());
    assert!(editor.insert_text(&copied, false));
    // Markers are regenerated from structure, so they must not enter the body.
    assert_eq!(
        editor.document().display_text(),
        "1. A\n2. B\n3. C\n4. B\n5. C"
    );

    let mut bullet = Editor::new(Document {
        format_version: 1,
        blocks: vec![Block::BulletList {
            items: vec![ListItem { text: "A".into() }],
        }],
    });
    bullet.end(false);
    bullet.enter();
    assert!(bullet.insert_text("• B\n• 中文", false));
    assert_eq!(bullet.document().display_text(), "• A\n• B\n• 中文");
    let revision = bullet.revision();
    assert!(!bullet.insert_text("• ", false));
    assert_eq!(bullet.revision(), revision);
}

#[test]
fn replacing_the_whole_list_preserves_literal_pasted_markers() {
    for text in ["1. A\n2. B", "• A\n• B"] {
        let mut editor = Editor::new(Document::default());
        editor.insert_text("1. ", true);
        editor.insert_text("old", true);
        let before = editor.document().clone();
        editor.select_all();
        assert!(editor.insert_text(text, false));
        assert_eq!(editor.document().display_text(), text);
        assert!(editor.rows().iter().all(|row| row.marker.is_empty()));
        editor.undo();
        assert_eq!(editor.document(), &before);
    }
}

#[test]
fn pasting_identical_text_moves_the_caret_without_a_revision_change() {
    let mut editor = Editor::new(Document {
        format_version: 1,
        blocks: vec![Block::Paragraph { text: "ab".into() }],
    });
    editor.select(
        Position { row: 0, offset: 1 },
        Position { row: 0, offset: 2 },
    );
    let revision = editor.revision();
    assert!(!editor.insert_text("b", true));
    assert_eq!(editor.selection().0, Position { row: 0, offset: 2 });
    assert_eq!(editor.selection().1, Position { row: 0, offset: 2 });
    assert_eq!(editor.revision(), revision);
    assert_eq!(editor.document().display_text(), "ab");
}

#[test]
fn merging_ordered_lists_preserves_continuations_and_independent_lists() {
    let ordered = |texts: &[&str], continuation| Block::OrderedList {
        items: texts
            .iter()
            .map(|text| ListItem {
                text: (*text).into(),
            })
            .collect(),
        continuation,
    };
    for second_items in [&["B"][..], &["B", "C"][..]] {
        let before = Document {
            format_version: 1,
            blocks: vec![
                ordered(&["A"], false),
                ordered(second_items, false),
                Block::Paragraph { text: "P".into() },
                ordered(&["D"], true),
                ordered(&["E", "F"], false),
            ],
        };
        before.validate().expect("valid ordered continuation");
        let mut editor = Editor::new(before.clone());
        let caret = Position { row: 0, offset: 1 };
        editor.select(caret, caret);

        assert!(editor.delete_forward());
        let expected = if second_items.len() == 1 {
            "1. AB\nP\n2. D\n1. E\n2. F"
        } else {
            "1. AB\n2. C\nP\n3. D\n1. E\n2. F"
        };
        assert_eq!(editor.document().display_text(), expected);
        editor.document().validate().expect("valid merged list");
        assert!(editor.undo());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.selection(), (caret, caret));
        assert!(editor.redo());
        assert_eq!(editor.document().display_text(), expected);
    }
}
