use filavim::{
    animation::{CursorTarget, Visuals},
    app::App,
    buffer::Buffer,
    config::Config,
    contextual::{self, Placement, Signature},
    contextual_ui,
    editor::{Editor, Mode},
    ide::{Choice, Panel, PanelKind, Row},
    storage::History,
    theme::Theme,
    workspace::{Axis, Workspace},
    workspace_ui,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Block,
};
use ropey::Rope;
use serde_json::json;
use std::{collections::BTreeMap, time::Instant};

fn app(root: &std::path::Path, text: &str) -> App {
    let mut config = Config::default();
    config.lsp.enabled = false;
    config.git.enabled = false;
    let mut editor = Editor::new(config);
    editor.set_buffer(Buffer::new(Rope::from_str(text), None));
    editor.buffer.cursor = text.chars().count();
    editor.set_mode(Mode::Insert);
    App::with_history_path(
        Workspace::new(editor, root.into()),
        History::default(),
        root.join("history.toml"),
    )
}
fn completion(app: &mut App, n: usize) {
    app.ide.panel = Some(Panel::list(
        "COMPLETION",
        PanelKind::Completion,
        (0..n)
            .map(|i| Row {
                label: format!("value{i:02}"),
                detail: String::new(),
                choice: Choice::Completion(json!({"label":format!("value{i:02}"),"kind":6})),
            })
            .collect(),
    ));
}
fn target(x: u16, y: u16, viewport: Rect) -> CursorTarget {
    CursorTarget {
        position: (x, y),
        viewport,
        origin: (0, 0),
        logical: 0,
        revision: 0,
    }
}
fn draw(app: &mut App, target: CursorTarget, size: (u16, u16)) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    terminal
        .draw(|f| {
            f.render_widget(
                Block::default().style(Style::default().bg(Theme::default().base)),
                f.area(),
            );
            contextual_ui::draw(f, app, &Theme::default(), Some(target));
        })
        .unwrap();
    terminal.backend().buffer().clone()
}
fn border(buffer: &ratatui::buffer::Buffer) -> (u16, u16) {
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            if buffer[(x, y)].symbol() == "╭" {
                return (x, y);
            }
        }
    }
    panic!("missing contextual popup border")
}

#[test]
fn popup_prefers_below_flips_above_and_clamps_to_text_viewport() {
    let v = Rect::new(8, 3, 40, 20);
    assert_eq!(
        contextual::place(v, (15, 8), 12, (22, 7), Placement::Below, &[]),
        Some(Rect::new(12, 9, 22, 7))
    );
    assert_eq!(
        contextual::place(v, (15, 21), 12, (22, 7), Placement::Below, &[]),
        Some(Rect::new(12, 14, 22, 7))
    );
    assert_eq!(
        contextual::place(v, (46, 8), 44, (22, 7), Placement::Below, &[]),
        Some(Rect::new(26, 9, 22, 7))
    );
    let clipped = contextual::place(
        Rect::new(3, 2, 9, 7),
        (8, 5),
        0,
        (60, 12),
        Placement::Below,
        &[],
    )
    .unwrap();
    assert_eq!(clipped, Rect::new(3, 6, 9, 3));
    assert!(
        contextual::place(
            Rect::new(0, 0, 3, 2),
            (1, 1),
            0,
            (20, 10),
            Placement::Below,
            &[]
        )
        .is_none()
    );
}

#[test]
fn signature_placement_avoids_completion_or_suppresses_itself() {
    let v = Rect::new(2, 2, 50, 20);
    let menu = contextual::place(v, (12, 10), 10, (25, 9), Placement::Below, &[]).unwrap();
    let sig = contextual::place(v, (12, 10), 11, (40, 3), Placement::Above, &[menu]).unwrap();
    assert!(sig.bottom() <= 10 && !sig.intersects(menu));
    assert!(
        contextual::place(
            Rect::new(0, 0, 30, 10),
            (2, 0),
            0,
            (25, 3),
            Placement::Above,
            &[Rect::new(0, 1, 30, 9)]
        )
        .is_none()
    );
}

#[test]
fn anchor_uses_unicode_cell_width_tabs_and_horizontal_scroll_origin() {
    let temp = tempfile::tempdir().unwrap();
    let app = app(temp.path(), "\t界🙂 value");
    let mut t = target(24, 8, Rect::new(10, 3, 40, 20));
    t.origin.0 = 5;
    // Four-column tab, two wide characters, and a space: word begins at column 9.
    assert_eq!(
        contextual_ui::completion_anchor(app.workspace.editor(), t),
        14
    );
}

#[test]
fn completion_is_compact_highlights_the_full_selected_row_and_scrolls() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(temp.path(), "va");
    completion(&mut app, 24);
    app.ide.panel.as_mut().unwrap().selected = 12;
    let v = Rect::new(5, 2, 40, 20);
    let output = draw(&mut app, target(7, 5, v), (60, 25));
    let (x, y) = border(&output);
    assert_eq!((x, y), (5, 6));
    let selected_y = y + 8;
    let row: String = (x + 1..x + 17)
        .map(|x| output[(x, selected_y)].symbol())
        .collect();
    assert!(row.contains("value12"), "{row}");
    for cell_x in x + 1..x + 17 {
        assert_eq!(output[(cell_x, selected_y)].bg, Theme::default().overlay);
    }
    for yy in 0..25 {
        for xx in 0..60 {
            if !v.contains((xx, yy).into()) {
                assert_eq!(output[(xx, yy)].symbol(), " ");
            }
        }
    }
}

#[test]
fn completion_size_and_anchor_remain_stable_as_results_change() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(temp.path(), "va");
    completion(&mut app, 12);
    let t = target(20, 16, Rect::new(4, 2, 40, 18));
    let first = draw(&mut app, t, (50, 24));
    let initial = border(&first);
    let size = app.ide.panel.as_ref().unwrap().popup_size;
    app.ide.panel.as_mut().unwrap().rows.truncate(2);
    let second = draw(&mut app, t, (50, 24));
    assert_eq!(border(&second), initial);
    assert_eq!(app.ide.panel.as_ref().unwrap().popup_size, size);
}

#[test]
fn signature_parameters_use_server_precedence_and_utf16_label_offsets() {
    let help = serde_json::from_value(json!({"activeSignature":1,"activeParameter":0,"signatures":[{"label":"other()"},{"label":"f(😀, value: i32) -> bool","activeParameter":1,"parameters":[{"label":[2,4]},{"label":[6,16]}]}]})).unwrap();
    let label = contextual::signature_label(&help).unwrap();
    assert_eq!(label.index, 1);
    assert_eq!(label.total, 2);
    assert_eq!(&label.label[label.active.unwrap()], "value: i32");
    let help = serde_json::from_value(json!({"activeParameter":1,"signatures":[{"label":"f(value, value)","parameters":[{"label":"value"},{"label":"value"}]}]})).unwrap();
    assert_eq!(
        contextual::signature_label(&help).unwrap().active,
        Some(9..14)
    );
    let malformed = serde_json::from_value(
        json!({"signatures":[{"label":"f(😀)","parameters":[{"label":[3,4]}]}]}),
    )
    .unwrap();
    assert!(
        contextual::signature_label(&malformed)
            .unwrap()
            .active
            .is_none()
    );
}

#[test]
fn compact_signature_highlights_only_active_parameter_and_stays_above_the_line() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(temp.path(), "f(1, ");
    app.ide.signature = Some(Signature { buffer:1, document:app.workspace.editor().document_id, call:None,
        help:serde_json::from_value(json!({"activeParameter":1,"signatures":[{"label":"f(first: i32, second: str) -> bool","parameters":[{"label":"first: i32"},{"label":"second: str"}]}]})).unwrap() });
    let output = draw(&mut app, target(10, 10, Rect::new(3, 2, 50, 19)), (60, 25));
    let (x, y) = border(&output);
    assert_eq!(y, 7);
    let active_x = x + 1 + "f(first: i32, ".len() as u16;
    assert_eq!(output[(active_x, y + 1)].fg, Theme::default().gold);
    assert!(output[(active_x, y + 1)].modifier.contains(Modifier::BOLD));
    assert_eq!(output[(x + 1, y + 1)].fg, Theme::default().text);
    assert_ne!(output[(x + 1, y + 1)].fg, Theme::default().gold);
}

#[test]
fn bounded_call_context_tracks_nested_calls_strings_commas_and_leaving_calls() {
    for (text, parameter) in [
        ("f(", 0),
        ("f(1, ", 1),
        ("f(g(1,2), ", 1),
        ("f([1,2], ", 1),
        ("f(\"a,b\", ", 1),
        ("f(/* , */ 1, ", 1),
        ("f(1,\n ", 1),
    ] {
        let r = Rope::from_str(text);
        assert_eq!(
            contextual::call_site(&r, r.len_chars()).unwrap().parameter,
            parameter,
            "{text}"
        );
    }
    for text in ["f(1)", "plain", "f(1); "] {
        let r = Rope::from_str(text);
        assert!(contextual::call_site(&r, r.len_chars()).is_none());
    }
}

#[test]
fn real_split_compositor_anchors_to_active_caret_and_preserves_other_view() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(temp.path(), &format!("{} va", "x".repeat(70)));
    app.workspace.split(Axis::Vertical);
    app.workspace.editor_mut().set_mode(Mode::Insert);
    let mut visuals = Visuals::new(app.workspace.config.effects());
    let theme = Theme::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let mut render = |app: &mut App| {
        terminal
            .draw(|f| {
                workspace_ui::draw(
                    f,
                    app,
                    &theme,
                    &BTreeMap::new(),
                    &mut visuals,
                    "",
                    Instant::now(),
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    };
    let plain = render(&mut app);
    completion(&mut app, 4);
    let popup = render(&mut app);
    assert!(app.workspace.editor().left > 0);
    let (x, y) = (0..24)
        .flat_map(|y| (50..100).map(move |x| (x, y)))
        .find(|&(x, y)| popup[(x, y)].symbol() == "╭" && plain[(x, y)] != popup[(x, y)])
        .expect("no completion in active split");
    assert!(x >= 50 && y >= 4, "popup {x},{y} escaped active split");
    for yy in 0..24 {
        for xx in 0..50 {
            assert_eq!(plain[(xx, yy)], popup[(xx, yy)]);
        }
    }
}

#[test]
fn unicode_long_signatures_and_tiny_viewports_are_clipped_without_panics() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(temp.path(), "界");
    completion(&mut app, 2);
    app.ide.signature = Some(Signature { buffer:1,document:app.workspace.editor().document_id,call:None,
        help:serde_json::from_value(json!({"signatures":[{"label":"function(界界界, 🙂🙂🙂, very_long_parameter: &[u8]) -> bool","parameters":[{"label":"very_long_parameter: &[u8]"}],"activeParameter":0}]})).unwrap() });
    for w in [1, 3, 6, 10, 18] {
        for h in [1, 3, 6, 12] {
            let output = draw(&mut app, target(0, h / 2, Rect::new(0, 0, w, h)), (w, h));
            assert_eq!(output.area, Rect::new(0, 0, w, h));
        }
    }
}
