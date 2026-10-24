use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use filavim::{
    buffer::{Buffer, display_width},
    config::{Config, Preset},
    editor::{Editor, Mode, Request},
    files,
    input::{Action, Keymap},
};
use ropey::Rope;

fn buffer(text: &str) -> Buffer {
    Buffer::new(Rope::from_str(text), None)
}
fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn unicode_clusters_delete_and_undo_without_corruption() {
    let mut b = buffer("a👩‍💻e\u{301}日本\r\nnext");
    let mut positions = vec![0];
    while b.cursor < b.text.len_chars() {
        b.horizontal(true);
        positions.push(b.cursor);
    }
    assert_eq!(positions, vec![0, 1, 4, 6, 7, 8, 10, 11, 12, 13, 14]);
    for &pos in positions.iter().rev().skip(1) {
        b.horizontal(false);
        assert_eq!(b.cursor, pos);
    }
    b.cursor = 4;
    b.backspace();
    assert_eq!(b.text.to_string(), "ae\u{301}日本\r\nnext");
    b.undo();
    assert_eq!(b.text.to_string(), "a👩‍💻e\u{301}日本\r\nnext");
    b.cursor = 8;
    b.delete();
    assert_eq!(b.text.to_string(), "a👩‍💻e\u{301}日本next");
}

#[test]
fn grapheme_boundaries_match_contiguous_unicode_across_rope_chunks() {
    use unicode_segmentation::UnicodeSegmentation;
    let cases = [
        format!(
            "{}👩‍💻{}e\u{301}\r\n{}🇵🇱🇺🇦",
            "x".repeat(1022),
            "é".repeat(1023),
            "日".repeat(1024)
        ),
        "🇵🇱".repeat(1500),
        format!("a{}b", "\u{301}".repeat(3000)),
        "\r\n".repeat(3000),
    ];
    for text in cases {
        let mut expected = vec![0];
        let mut at = 0;
        for g in text.graphemes(true) {
            at += g.chars().count();
            expected.push(at);
        }
        let mut b = buffer(&text);
        for &pos in &expected[1..] {
            b.horizontal(true);
            assert_eq!(b.cursor, pos);
        }
        for &pos in expected.iter().rev().skip(1) {
            b.horizontal(false);
            assert_eq!(b.cursor, pos);
        }
    }
}
#[test]
fn vertical_navigation_preserves_display_column_across_short_lines() {
    let mut b = buffer("\t日本x\na\n\t日本x");
    b.cursor = 3;
    assert_eq!(b.column(4), 8);
    b.vertical(1, 4);
    assert_eq!(b.cursor, 6); // end of short line
    b.vertical(1, 4);
    assert_eq!(b.column(4), 8);
    b.home();
    b.vertical(-1, 4);
    assert_eq!(b.column(4), 0);
    assert_eq!(display_width("a\t日本👩‍💻", 4), 10);
}
#[test]
fn undo_groups_saved_identity_and_redo_branch() {
    let mut b = buffer("");
    b.begin_group();
    b.insert("h");
    b.insert("i");
    b.end_group();
    let saved = b.state();
    b.mark_saved(saved);
    assert!(!b.dirty());
    b.insert("!");
    assert!(b.dirty());
    b.undo();
    assert_eq!(b.text.to_string(), "hi");
    assert!(!b.dirty());
    b.undo();
    assert_eq!(b.text.to_string(), "");
    assert!(b.dirty());
    b.redo();
    assert!(!b.dirty());
    b.insert("?");
    assert!(!b.redo());
}
#[test]
fn indentation_crlf_and_terminal_paste() {
    let mut b = buffer("  one\r\n  two\r\n");
    assert_eq!(b.newline, "\r\n");
    b.end();
    b.newline(true);
    assert!(b.text.to_string().starts_with("  one\r\n  \r\n"));
    b.insert_paste("a\nb\r\nc");
    assert!(b.text.to_string().contains("a\r\nb\r\nc"));
}
#[test]
fn modes_selection_copy_cut_paste_and_insert_undo() {
    let mut e = Editor::new(Config::default());
    e.act(Action::Insert);
    e.type_text("hello 日本");
    e.act(Action::Normal);
    assert_eq!(e.mode, Mode::Normal);
    assert!(!e.dashboard);
    e.act(Action::Home);
    e.act(Action::Visual);
    e.act(Action::Right);
    e.act(Action::Right);
    assert_eq!(e.selection(), Some(0..3));
    assert!(matches!(e.act(Action::Cut),Some(Request::Clipboard(s)) if s == "hel"));
    assert_eq!(e.buffer.text.to_string(), "lo 日本");
    e.act(Action::Paste);
    assert_eq!(e.buffer.text.to_string(), "hello 日本");
    e.act(Action::Undo);
    assert_eq!(e.buffer.text.to_string(), "lo 日本");
    e.act(Action::Undo);
    assert_eq!(e.buffer.text.to_string(), "hello 日本");
    e.act(Action::Undo);
    assert_eq!(e.buffer.text.to_string(), "");
}
#[test]
fn delete_final_line_removes_its_preceding_line_break() {
    let mut e = Editor::new(Config::default());
    e.set_buffer(buffer("one\r\ntwo"));
    e.act(Action::Bottom);
    e.act(Action::DeleteLine);
    assert_eq!(e.buffer.text.to_string(), "one");
    e.act(Action::Undo);
    assert_eq!(e.buffer.text.to_string(), "one\r\ntwo");
}
#[test]
fn commands_guard_changes_and_return_typed_io_requests() {
    let mut e = Editor::new(Config::default());
    e.act(Action::Insert);
    e.type_text("content");
    e.act(Action::Normal);
    assert!(e.execute_command("q").is_none());
    assert!(e.error);
    assert!(e.execute_command("e another.rs").is_none());
    assert!(e.execute_command("new").is_none());
    assert!(e.buffer.dirty());
    assert!(
        matches!(e.execute_command("wq \"path with spaces.rs\""),Some(Request::Save { path,quit: true,.. }) if path.to_str() == Some("path with spaces.rs"))
    );
    assert!(matches!(
        e.execute_command("e! new.rs"),
        Some(Request::Open { .. })
    ));
    assert!(matches!(e.execute_command("q!"), Some(Request::Quit)));
    let id = e.document_id;
    e.execute_command("new!");
    assert_eq!(e.document_id, id + 1);
    assert!(!e.buffer.dirty());
    e.execute_command("e '"); // malformed quotation must not panic
}
#[test]
fn command_input_backspace_is_grapheme_aware() {
    let mut e = Editor::new(Config::default());
    e.act(Action::Command);
    e.type_text("e café👩‍💻");
    e.act(Action::Backspace);
    assert_eq!(e.command, "e café");
    let request = e.act(Action::Submit);
    assert!(matches!(request, Some(Request::Open { .. })));
    assert_eq!(e.mode, Mode::Normal);
    e.act(Action::Search);
    e.type_text("café");
    assert!(matches!(
        e.act(Action::Submit),
        Some(Request::Search { .. })
    ));
}
#[test]
fn keymap_is_mode_aware_and_supports_remapping_and_leader() {
    let config =
        Config::parse("[keybindings.normal]\nf = 'right'\nd = 'noop'\n'space s' = 'save'\n")
            .unwrap();
    let mut map = Keymap::new(&config).unwrap();
    assert_eq!(map.resolve(Mode::Normal, key('w')), Some(Action::Up));
    assert_eq!(map.resolve(Mode::Insert, key('w')), None);
    assert_eq!(map.resolve(Mode::Normal, key('f')), Some(Action::Right));
    assert_eq!(map.resolve(Mode::Normal, key('d')), Some(Action::Noop));
    assert_eq!(map.resolve(Mode::Normal, key(' ')), Some(Action::Noop));
    assert_eq!(map.resolve(Mode::Normal, key('s')), Some(Action::Save));
    assert_eq!(map.resolve(Mode::Normal, key(':')), Some(Action::Command));
    assert_eq!(
        map.resolve(
            Mode::Normal,
            KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)
        ),
        Some(Action::Append)
    );
    map.resolve(Mode::Normal, key(' '));
    map.resolve(
        Mode::Normal,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(map.pending().is_empty());
}
#[test]
fn configuration_validates_typos_and_theme_values() {
    assert_eq!(
        Config::parse("preset = 'minimal'").unwrap().preset,
        Preset::Minimal
    );
    for text in [
        "tab_width = 0",
        "tab_width = 99",
        "unknown = true",
        "[theme]\nbase = '#zzzzzz'",
        "[theme]\nbogus = '#ffffff'",
        "[keybindings.normal]\nw = 'fly'",
        "[keybindings.nromal]\nw = 'up'",
    ] {
        assert!(Config::parse(text).is_err(), "accepted {text}");
    }
}
#[test]
fn disk_roundtrip_new_file_overwrite_invalid_utf8_and_missing_parent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("hello.txt");
    assert_eq!(files::open(&path).unwrap().len_chars(), 0);
    let original = Rope::from_str("\u{feff}日本\r\none\r\n");
    files::save(&path, &original).unwrap();
    assert_eq!(files::open(&path).unwrap(), original);
    files::save(&path, &Rope::from_str("replaced")).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "replaced");
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(files::open(&path).is_err());
    assert!(files::open(&temp.path().join("missing/child.txt")).is_err());
    assert!(files::open(temp.path()).is_err());
    assert!(files::save(temp.path(), &original).is_err());
}
#[test]
fn literal_search_wraps_unicode_and_matches_across_rope_chunks() {
    let text = Rope::from_str("a日本 a日本 aaaab");
    assert_eq!(files::find(&text, "日本", 0), Some(1));
    assert_eq!(files::find(&text, "日本", 3), Some(5));
    assert_eq!(files::find(&text, "日本", 8), Some(1));
    assert_eq!(files::find(&text, "aaab", 0), Some(9));
    assert_eq!(files::find(&text, "", 0), None);
    assert_eq!(files::find(&text, "missing", 0), None);
    let long = Rope::from_str(&format!("{}needle{}", "x".repeat(8191), "x".repeat(8191)));
    assert_eq!(files::find(&long, "needle", 0), Some(8191));
}

#[test]
fn rope_grapheme_stream_matches_unicode_segmentation_across_chunks() {
    use unicode_segmentation::UnicodeSegmentation;
    let source = format!(
        "{}{}",
        "a".repeat(991),
        "🇵🇱e\u{301}👨‍👩‍👦\t日本\r\n".repeat(120)
    );
    let rope = Rope::from_str(&source);
    let actual: Vec<_> = filavim::buffer::rope_graphemes(rope.slice(..))
        .map(|g| g.into_owned())
        .collect();
    let expected = source.graphemes(true).collect::<Vec<_>>();
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(a, b, "cluster {i}");
    }
    assert_eq!(actual.len(), expected.len());
    let mut buffer = Buffer::new(rope, None);
    for _ in 0..300 {
        buffer.insert("z");
    }
    assert!(buffer.changes_since(0).is_none());
    assert_eq!(buffer.changes_since(299).unwrap().len(), 1);
}
