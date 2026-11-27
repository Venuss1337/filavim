#[cfg(feature = "native")]
use filavim::gui::input::translate;
use filavim::{
    buffer::Buffer,
    config::Config,
    editor::{Mode, Request},
    gui::{
        FONT_BOLD_BYTES, FONT_BYTES, FONT_FAMILY,
        io::{self, Job},
        model::Model,
        viewport::VisibleLine,
    },
    input::Action,
    key::{KeyCode, KeyEvent, KeyModifiers},
};
#[cfg(feature = "native")]
use gpui::{Keystroke, Modifiers};
use ropey::Rope;
fn model(text: &str) -> Model {
    let mut m = Model::new(Config::default()).unwrap();
    m.set_buffer(Buffer::new(Rope::from_str(text), None));
    m
}
fn key(m: &mut Model, c: char) {
    assert!(m.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)).0);
}
#[test]
fn bundled_agave_is_the_exact_mono_variant_and_has_nerd_glyphs() {
    let f = ttf_parser::Face::parse(FONT_BYTES, 0).unwrap();
    let names: Vec<_> = f
        .names()
        .into_iter()
        .filter_map(|n| n.to_string())
        .collect();
    assert!(names.iter().any(|s| s == FONT_FAMILY), "{names:?}");
    assert!(f.is_monospaced());
    for c in ['A', 'é', '\u{e0b0}', '\u{f120}', '\u{f07b}'] {
        assert!(f.glyph_index(c).is_some(), "missing {c}");
    }
    let a = f.glyph_hor_advance(f.glyph_index('i').unwrap());
    assert_eq!(a, f.glyph_hor_advance(f.glyph_index('W').unwrap()));
    // The bold face is the same family, heavier, and on the same monospace grid,
    // so bold labels and computed tab widths stay column-aligned.
    let b = ttf_parser::Face::parse(FONT_BOLD_BYTES, 0).unwrap();
    let bold_names: Vec<_> = b
        .names()
        .into_iter()
        .filter_map(|n| n.to_string())
        .collect();
    assert!(
        bold_names.iter().any(|s| s == FONT_FAMILY),
        "{bold_names:?}"
    );
    assert!(b.is_bold() && b.is_monospaced() && !f.is_bold());
    assert!(b.weight().to_number() > f.weight().to_number());
    let grid = |face: &ttf_parser::Face| {
        f32::from(
            face.glyph_hor_advance(face.glyph_index('m').unwrap())
                .unwrap(),
        ) / f32::from(face.units_per_em())
    };
    assert_eq!(grid(&f), grid(&b));
    for c in ['A', '\u{e0b0}', '\u{f07b}'] {
        assert!(b.glyph_index(c).is_some(), "bold missing {c}");
    }
}
#[test]
fn gpui_keys_use_existing_modes_leader_and_undo() {
    let mut m = model("one\ntwo");
    key(&mut m, 'd');
    assert_eq!(m.editor.buffer.cursor, 1);
    key(&mut m, 'v');
    key(&mut m, 'd');
    assert_eq!(m.editor.selection(), Some(1..3));
    key(&mut m, 'x');
    assert_eq!(m.editor.buffer.text.to_string(), "o\ntwo");
    key(&mut m, 'u');
    assert_eq!(m.editor.buffer.text.to_string(), "one\ntwo");
    key(&mut m, 'i');
    m.replace_input(None, "日本👩‍💻e\u{301}", false, None);
    m.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    key(&mut m, 'u');
    assert_eq!(m.editor.buffer.text.to_string(), "one\ntwo");
    let (_, r) = m.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(r.is_none());
    assert!(m.editor.buffer.text.to_string().contains("日本"));
    key(&mut m, ' ');
    let (_, r) = m.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    assert!(r.is_none());
    assert!(m.editor.error); // unnamed save, not a second buffer
}
#[test]
#[cfg(feature = "native")]
fn native_keystrokes_distinguish_commands_shift_altgr_and_text() {
    let k = Keystroke {
        key: "i".into(),
        key_char: Some("I".into()),
        modifiers: Modifiers {
            shift: true,
            ..Default::default()
        },
    };
    let k = translate(&k).unwrap();
    assert_eq!(k.code, KeyCode::Char('I'));
    assert!(k.modifiers.contains(KeyModifiers::SHIFT));
    let k = Keystroke {
        key: ";".into(),
        key_char: Some(":".into()),
        modifiers: Modifiers {
            shift: true,
            ..Default::default()
        },
    };
    let mut m = model("");
    assert!(m.key(translate(&k).unwrap()).0);
    assert_eq!(m.editor.mode, Mode::Command);
    let k = Keystroke {
        key: "a".into(),
        key_char: Some("ą".into()),
        modifiers: Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        },
    };
    m.editor.set_mode(Mode::Insert);
    assert!(!m.key(translate(&k).unwrap()).0);
    m.replace_input(None, "ą", false, None);
    assert_eq!(m.editor.buffer.text.to_string(), "ą");
}
#[test]
fn utf16_composition_updates_real_rope_and_is_one_undo_group() {
    let mut m = model("a😀b\r\n");
    assert_eq!(m.from_utf16(2), 1);
    assert_eq!(m.from_utf16(3), 2);
    assert_eq!(m.to_utf16(2), 3);
    m.editor.buffer.cursor = 2;
    m.editor.set_mode(Mode::Insert);
    m.replace_input(None, "に", true, Some(1..1));
    assert_eq!(m.marked, Some(2..3));
    m.replace_input(None, "日本", true, Some(2..2));
    assert_eq!(m.editor.buffer.text.to_string(), "a😀日本b\r\n");
    m.replace_input(None, "日本語", false, None);
    assert!(m.marked.is_none());
    assert_eq!(m.editor.buffer.cursor, 5);
    assert_eq!(m.selected_utf16(), 6..6);
    let (r, s) = m.text_utf16(1..3);
    assert_eq!(r, 1..3);
    assert_eq!(s, "😀");
    m.editor.set_mode(Mode::Normal);
    m.act(Action::Undo);
    assert_eq!(m.editor.buffer.text.to_string(), "a😀b\r\n");
    m.act(Action::Redo);
    assert_eq!(m.editor.buffer.text.to_string(), "a😀日本語b\r\n");
}
#[test]
fn normal_mode_rejects_text_and_command_ime_has_separate_coordinates() {
    let mut m = model("original");
    m.replace_input(None, "wrong", false, None);
    assert_eq!(m.editor.buffer.text.to_string(), "original");
    m.act(Action::Command);
    m.replace_input(None, "e ", false, None);
    m.replace_input(None, "に", true, None);
    m.replace_input(None, "日本.txt", false, None);
    assert_eq!(m.editor.command, "e 日本.txt");
    assert!(m.marked.is_none());
    let r = m.act(Action::Submit);
    assert!(matches!(r, Some(Request::Open { .. })));
    assert_eq!(m.editor.buffer.text.to_string(), "original");
}
#[test]
fn visible_fragments_map_tabs_graphemes_crlf_and_horizontal_clipping() {
    let m = model("\tAe\u{301}👩‍💻日本\r\nsecond");
    let l = VisibleLine::new(&m.editor.buffer, 0, 0, 80, 4);
    assert_eq!(l.text, "    Ae\u{301}👩‍💻日本");
    assert_eq!(l.byte_for_char(1), 4);
    assert_eq!(l.char_for_byte(3), 0);
    assert_eq!(l.char_for_byte(l.byte_for_char(7)), 7);
    assert_eq!(l.end, 9);
    let tail = VisibleLine::new(&m.editor.buffer, 0, 7, 4, 4);
    assert!(tail.start > 0);
    assert!(!tail.text.contains('\r'));
    assert!(
        tail.boundaries
            .windows(2)
            .all(|p| p[0].0 < p[1].0 && p[0].1 < p[1].1)
    );
}
#[test]
fn viewport_work_is_bounded_for_many_lines_and_long_ascii_lines() {
    let m = model(&format!(
        "{}\n{}",
        "x".repeat(1_000_000),
        "line\n".repeat(100_000)
    ));
    for left in [0, 500_000, 999_900] {
        let l = VisibleLine::new(&m.editor.buffer, 0, left, 100, 4);
        assert!(l.text.len() <= 102);
        assert!(l.boundaries.len() <= 103);
        assert_eq!(l.start, left);
    }
    let l = VisibleLine::new(&m.editor.buffer, 99_999, 0, 100, 4);
    assert_eq!(l.text, "line");
}
#[test]
fn save_snapshot_does_not_mark_newer_edits_clean_and_open_rejects_stale_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    let mut m = model("a😀\r\n");
    m.editor.buffer.insert("!");
    let request = m.editor.save(Some(path.clone()), true).unwrap();
    let done = Job::from_request(request, m.editor.document_id)
        .unwrap()
        .run()
        .unwrap();
    m.editor.buffer.insert("newer");
    assert!(!io::apply(&mut m.editor, done));
    assert!(m.editor.buffer.dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "!a😀\r\n");
    let open = Job::Open {
        path: path.clone(),
        document: m.editor.document_id,
        revision: m.editor.buffer.revision,
    }
    .run()
    .unwrap();
    m.editor.buffer.insert("changed");
    io::apply(&mut m.editor, open);
    assert!(m.editor.buffer.text.to_string().contains("changed"));
    let done = Job::from_request(m.editor.save(None, false).unwrap(), m.editor.document_id)
        .unwrap()
        .run()
        .unwrap();
    io::apply(&mut m.editor, done);
    assert!(!m.editor.buffer.dirty());
}
#[test]
fn new_document_invalidates_inflight_open_and_save_metadata() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    std::fs::write(&p, "on disk").unwrap();
    let mut m = model("old");
    let opened = Job::Open {
        path: p.clone(),
        document: m.editor.document_id,
        revision: 0,
    }
    .run()
    .unwrap();
    let saved = Job::from_request(m.editor.save(Some(p), true).unwrap(), m.editor.document_id)
        .unwrap()
        .run()
        .unwrap();
    m.set_buffer(Buffer::default());
    io::apply(&mut m.editor, opened);
    assert_eq!(m.editor.buffer.text.len_chars(), 0);
    assert!(!io::apply(&mut m.editor, saved));
    assert!(m.editor.buffer.path.is_none());
}
#[test]
fn close_and_open_commands_guard_unsaved_documents() {
    let mut m = model("");
    key(&mut m, 'i');
    m.replace_input(None, "unsaved", false, None);
    m.act(Action::Normal);
    assert!(m.editor.execute_command("q").is_none());
    assert!(m.editor.error);
    assert!(m.editor.execute_command("e other.txt").is_none());
    assert!(m.editor.buffer.dirty());
    assert!(matches!(
        m.editor.execute_command("q!"),
        Some(Request::Quit)
    ));
}
#[test]
#[ignore = "individual-sample native boundary performance baseline; run release serially"]
fn native_boundary_performance() {
    use std::time::Instant;
    let mut m = model(&format!(
        "{}\n{}",
        "x".repeat(1_000_000),
        "line\n".repeat(100_000)
    ));
    m.editor.buffer.cursor = 500_000;
    m.editor.buffer.column(4); // warm sparse checkpoints
    let mut samples = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let start = Instant::now();
        m.editor.buffer.horizontal(i % 2 == 0);
        let col = m.editor.buffer.column(4);
        std::hint::black_box(VisibleLine::new(
            &m.editor.buffer,
            0,
            col.saturating_sub(80),
            120,
            4,
        ));
        samples.push(start.elapsed().as_nanos());
    }
    samples.sort_unstable();
    eprintln!(
        "navigation_and_fragment n={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        samples.len(),
        samples[5000] as f64 / 1000.,
        samples[9500] as f64 / 1000.,
        samples[9900] as f64 / 1000.,
        samples[9999] as f64 / 1000.
    );
    m.editor.set_mode(Mode::Insert);
    samples.clear();
    for i in 0..10_000 {
        let start = Instant::now();
        m.replace_input(None, ["x", "😀", "e\u{301}"][i % 3], false, None);
        let col = m.editor.buffer.column(4);
        std::hint::black_box(VisibleLine::new(
            &m.editor.buffer,
            0,
            col.saturating_sub(80),
            120,
            4,
        ));
        samples.push(start.elapsed().as_nanos());
    }
    samples.sort_unstable();
    eprintln!(
        "typing_and_fragment n={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        samples.len(),
        samples[5000] as f64 / 1000.,
        samples[9500] as f64 / 1000.,
        samples[9900] as f64 / 1000.,
        samples[9999] as f64 / 1000.
    );
}

#[test]
fn broken_surrogate_ranges_are_adjusted_and_multiline_ime_keeps_crlf_coordinates() {
    let mut m = model("a😀b\r\n");
    let (r, text) = m.text_utf16(2..3);
    assert_eq!(r, 1..3);
    assert_eq!(text, "😀");
    m.editor.set_mode(Mode::Command);
    m.replace_input(
        Some(std::ops::Range { start: 10, end: 0 }),
        "w",
        false,
        None,
    );
    assert_eq!(m.editor.command, "w");
    m.editor.set_mode(Mode::Insert);
    m.editor.buffer.cursor = 0;
    m.replace_input(None, "x\ny", true, Some(3..3));
    assert!(m.editor.buffer.text.to_string().starts_with("x\r\ny"));
    assert_eq!(m.editor.buffer.cursor, 4);
    assert_eq!(m.marked, Some(0..4));
    m.replace_input(None, "done", false, None);
    assert!(m.editor.buffer.text.to_string().starts_with("donea😀b"));
}

#[test]
fn empty_composition_cancellation_does_not_disable_modal_key_dispatch() {
    let mut m = model("original");
    m.act(Action::Insert);
    m.replace_input(None, "preedit", true, None);
    m.replace_input(None, "", true, None);
    assert!(m.marked.is_none());
    assert_eq!(m.editor.buffer.text.to_string(), "original");
    assert!(m.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).0);
    assert_eq!(m.editor.mode, Mode::Normal);
}
