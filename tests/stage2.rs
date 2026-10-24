use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use filavim::{
    animation::Visuals,
    app::{App, PickerKind},
    background::LatestWorker,
    buffer::Buffer,
    config::{Config, Preset},
    editor::{Editor, Mode},
    input::{Action, Keymap},
    navigation::{self, Item, PreviewJob, TreeJob},
    storage::{History, Mutation, mutate},
    theme::Theme,
    workspace::{Axis, Workspace, absolute, project_root},
    workspace_ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use ropey::Rope;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

fn workspace(root: &Path, text: &str) -> Workspace {
    let mut e = Editor::new(Config {
        preset: Preset::Minimal,
        ..Config::default()
    });
    e.set_buffer(Buffer::new(Rope::from_str(text), None));
    Workspace::new(e, root.to_path_buf())
}
fn wait(app: &mut App, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.poll();
        if condition(app) {
            return;
        }
        thread::sleep(Duration::from_millis(3));
    }
    panic!(
        "async operation did not finish: {}",
        app.workspace.editor().message
    );
}
fn app(root: &Path) -> App {
    App::with_history_path(
        workspace(root, ""),
        History::default(),
        root.join(".test-state/history.toml"),
    )
}
fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
#[test]
fn fuzzy_ranking_has_stable_order_and_unicode_highlights() {
    let items = [
        "tests/main_tests.rs",
        "src/main.rs",
        "src/日本.rs",
        "docs/README.md",
    ]
    .map(|label| Item::file(label.into(), label.into()));
    let matches = navigation::rank(&items, "main", 20, || false);
    assert_eq!(matches[0].item.label, "src/main.rs");
    assert!(!matches[0].indices.is_empty());
    let matches = navigation::rank(&items, "日本", 20, || false);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].indices, vec![4, 5]);
    let empty = navigation::rank(&items, "", 2, || false);
    assert_eq!(empty[0].item.label, items[0].label);
    assert_eq!(empty.len(), 2);
    assert!(navigation::rank(&items, "main", 20, || true).is_empty());
}
#[test]
fn indexing_respects_ignores_hidden_and_never_descends_git() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), "secret").unwrap();
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("ignored.txt"), "ignored").unwrap();
    fs::write(root.join(".hidden"), "hidden").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    let c = Config::default().navigation;
    let index = navigation::build_index(root.clone(), &c, || false);
    assert_eq!(index.files.len(), 1);
    assert_eq!(index.files[0].label, "src/main.rs");
    assert!(
        index
            .entries
            .iter()
            .any(|e| e.label == "empty" && e.directory)
    );
    let c = filavim::config::NavigationConfig {
        show_hidden: true,
        respect_ignore: false,
        ..c
    };
    let index = navigation::build_index(root, &c, || false);
    assert!(index.files.iter().any(|i| i.label == "ignored.txt"));
    assert!(index.files.iter().any(|i| i.label == ".hidden"));
    assert!(!index.files.iter().any(|i| i.label.starts_with(".git/")));
}
#[test]
fn regex_grep_is_smart_case_unicode_bounded_and_skips_binary() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::write(root.join("a.txt"), "hello 日本\nHELLO world\n").unwrap();
    fs::write(root.join("binary.bin"), b"\0hello").unwrap();
    fs::write(root.join("invalid.txt"), [0xff, 0xfe, b'\n']).unwrap();
    let c = Config::default().navigation;
    let index = navigation::build_index(root, &c, || false);
    let results = navigation::grep(&index, "hello", &c, || false);
    assert_eq!(results.matches.len(), 2);
    let results = navigation::grep(&index, "HELLO", &c, || false);
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].item.line, 1);
    let results = navigation::grep(&index, "日本", &c, || false);
    assert_eq!(results.matches[0].item.column, 6);
    assert!(!results.matches[0].indices.is_empty());
    assert!(
        navigation::grep(&index, "[", &c, || false)
            .message
            .contains("Invalid regex")
    );
    let c = filavim::config::NavigationConfig {
        max_results: 1,
        ..c
    };
    assert_eq!(
        navigation::grep(&index, "hello", &c, || false)
            .matches
            .len(),
        1
    );
}
#[test]
fn workers_cancel_old_results_and_only_publish_latest() {
    let (started_tx, started_rx) = mpsc::channel();
    let worker = LatestWorker::new(move |job: u32, cancel: &filavim::background::Cancel| {
        if job == 1 {
            started_tx.send(()).unwrap();
            while !cancel.cancelled() {
                thread::yield_now();
            }
        }
        job
    });
    worker.submit(1);
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.submit(2);
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = loop {
        if let Some(result) = worker.poll() {
            break result;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(result, 2);
}
#[test]
fn previews_show_match_lines_unsaved_text_and_binary_fallback() {
    let temp = root();
    let path = temp.path().join("file.txt");
    fs::write(&path, "first\n日本 match\nthird").unwrap();
    let worker = LatestWorker::new(navigation::preview);
    let mut item = Item::file(path.clone(), "file.txt".into());
    item.line = 1;
    worker.submit(PreviewJob {
        item: item.clone(),
        query: "日本".into(),
        max_bytes: 1024,
        text: None,
    });
    let poll = || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.poll() {
                break result;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
    };
    let p = poll();
    assert_eq!(p.focused_line, 1);
    assert_eq!(p.ranges[0], (1, 0..6));
    worker.submit(PreviewJob {
        item: item.clone(),
        query: String::new(),
        max_bytes: 1024,
        text: Some(Rope::from_str("unsaved\n日本")),
    });
    assert_eq!(poll().lines[0], "unsaved");
    fs::write(path, b"\0binary").unwrap();
    worker.submit(PreviewJob {
        item,
        query: String::new(),
        max_bytes: 1024,
        text: None,
    });
    assert!(poll().message.contains("Binary"));
}
#[test]
fn buffers_keep_undo_and_modified_text_and_share_registers() {
    let mut w = workspace(Path::new("."), "abc");
    w.apply(Action::Insert);
    w.type_text("X");
    w.apply(Action::Normal);
    assert!(w.editor().buffer.dirty());
    let second = w.add_buffer(Buffer::default());
    assert_eq!(second, 2);
    w.switch_buffer(1);
    assert_eq!(w.editor().buffer.text.to_string(), "Xabc");
    w.apply(Action::Copy);
    w.switch_buffer(second);
    w.apply(Action::Paste);
    assert_eq!(w.editor().buffer.text.to_string(), "Xabc");
    w.switch_buffer(1);
    w.apply(Action::Undo);
    assert_eq!(w.editor().buffer.text.to_string(), "abc");
    assert!(w.close_buffer(second, false).is_err());
    w.close_buffer(second, true).unwrap();
    assert_eq!(w.buffers.len(), 1);
}
#[test]
fn split_views_have_independent_positions_and_shared_edits() {
    let mut w = workspace(Path::new("."), "abc\nlong line\nz");
    w.editor_mut().buffer.cursor = 2;
    w.stash();
    let first = w.focused();
    w.split(Axis::Vertical);
    let second = w.focused();
    w.apply(Action::Left);
    assert_eq!(w.views[&first].cursor, 2);
    assert_eq!(w.views[&second].cursor, 1);
    w.apply(Action::Insert);
    w.type_text("日");
    w.apply(Action::Normal);
    assert_eq!(w.views[&first].cursor, 3);
    assert_eq!(w.views[&second].cursor, 2);
    w.switch_view(first);
    assert_eq!(w.editor().buffer.cursor, 3);
    assert!(w.editor().buffer.text.to_string().starts_with("a日bc"));
    w.views.get_mut(&second).unwrap().top = 2;
    w.switch_view(second);
    assert_eq!(w.editor().top, 2);
    w.switch_view(first);
    assert_eq!(w.editor().top, 0);
}
#[test]
fn visual_terminal_paste_is_one_undo_step_and_adjusts_other_views() {
    let mut w = workspace(Path::new("."), "abcdef");
    w.editor_mut().buffer.cursor = 6;
    w.stash();
    let first = w.focused();
    w.split(Axis::Vertical);
    w.apply(Action::Home);
    w.apply(Action::Visual);
    w.apply(Action::Right);
    w.apply(Action::Right);
    w.paste("日本");
    assert_eq!(w.editor().buffer.text.to_string(), "日本def");
    assert_eq!(w.views[&first].cursor, 5);
    w.apply(Action::Normal);
    w.apply(Action::Undo);
    assert_eq!(w.editor().buffer.text.to_string(), "abcdef");
    assert!(!w.editor().buffer.dirty());
}
#[test]
fn nested_splits_resize_cover_screen_and_tabs_preserve_layouts() {
    let mut w = workspace(Path::new("."), "hello");
    w.split(Axis::Vertical);
    w.split(Axis::Horizontal);
    let mut layout = Vec::new();
    w.tabs[0]
        .layout
        .layout(Rect::new(0, 0, 100, 40), &mut layout);
    assert_eq!(layout.len(), 3);
    assert_eq!(
        layout
            .iter()
            .map(|(_, r)| u32::from(r.width) * u32::from(r.height))
            .sum::<u32>(),
        4000
    );
    let before = layout.iter().find(|(id, _)| *id == w.focused()).unwrap().1;
    w.resize(5);
    layout.clear();
    w.tabs[0]
        .layout
        .layout(Rect::new(0, 0, 100, 40), &mut layout);
    assert_ne!(
        layout.iter().find(|(id, _)| *id == w.focused()).unwrap().1,
        before
    );
    let original = w.focused();
    w.new_tab();
    assert_eq!(w.tabs.len(), 2);
    w.apply(Action::Right);
    w.switch_tab(0);
    assert_eq!(w.focused(), original);
    assert!(w.close_view());
    assert_eq!(w.views.len(), 3);
    w.switch_tab(1);
    assert!(w.close_view());
    assert_eq!(w.tabs.len(), 1);
}
#[test]
fn explorer_tree_expansion_is_cached_and_deterministic() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/a.rs"), "a").unwrap();
    fs::write(root.join("README"), "readme").unwrap();
    let index = Arc::new(navigation::build_index(
        root,
        &Config::default().navigation,
        || false,
    ));
    let worker = LatestWorker::new(navigation::tree);
    worker.submit(TreeJob {
        index: index.clone(),
        expanded: BTreeSet::new(),
    });
    let poll = || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(rows) = worker.poll() {
                break rows;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
    };
    let rows = poll();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].entry.directory);
    worker.submit(TreeJob {
        index,
        expanded: BTreeSet::from(["src".into()]),
    });
    let rows = poll();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].depth, 1);
    assert_eq!(rows[1].entry.label, "src/a.rs");
}
#[test]
fn filesystem_mutations_stay_inside_root_and_do_not_overwrite() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    let a = root.join("a");
    let b = root.join("b");
    mutate(
        &root,
        &Mutation::Create {
            path: a.clone(),
            directory: true,
        },
    )
    .unwrap();
    fs::write(a.join("file"), "data").unwrap();
    mutate(
        &root,
        &Mutation::Rename {
            from: a,
            to: b.clone(),
        },
    )
    .unwrap();
    assert_eq!(fs::read_to_string(b.join("file")).unwrap(), "data");
    assert!(
        mutate(
            &root,
            &Mutation::Create {
                path: b.clone(),
                directory: false
            }
        )
        .is_err()
    );
    assert!(mutate(&root, &Mutation::Delete { path: root.clone() }).is_err());
    let outside = root.parent().unwrap().join("outside-filavim-test");
    assert!(
        mutate(
            &root,
            &Mutation::Create {
                path: outside,
                directory: false
            }
        )
        .is_err()
    );
    mutate(&root, &Mutation::Delete { path: b.clone() }).unwrap();
    assert!(!b.exists());
}
#[test]
fn mru_history_roundtrips_is_unique_and_is_bounded() {
    let temp = root();
    let mut h = History::default();
    for i in 0..120 {
        h.file(format!("file{i}.rs").into());
    }
    h.file("file119.rs".into());
    assert_eq!(h.files.len(), 100);
    assert_eq!(h.files[0], Path::new("file119.rs"));
    h.project(temp.path().into());
    let path = temp.path().join("state/history.toml");
    h.save_to(&path).unwrap();
    let loaded: History = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(loaded.files, h.files);
    assert_eq!(loaded.projects, h.projects);
}
#[test]
fn root_detection_and_normalization_do_not_require_git() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::write(root.join("Cargo.toml"), "").unwrap();
    fs::create_dir(root.join("src")).unwrap();
    assert_eq!(project_root(&root.join("src")), root);
    assert_eq!(
        absolute(&root, Path::new("src/../new.rs")),
        root.join("new.rs")
    );
}
#[test]
fn stage2_leaders_are_central_remappable_and_have_no_prefix_conflicts() {
    Config::parse(include_str!("../config.example.toml")).unwrap();
    let mut map = Keymap::new(&Config::default()).unwrap();
    for (keys, expected) in [
        (" ff", Action::FindFiles),
        (" fg", Action::LiveGrep),
        (" fr", Action::RecentFiles),
        (" bb", Action::Buffers),
        (" e", Action::Explorer),
        (" sv", Action::SplitVertical),
        (" sh", Action::SplitHorizontal),
        (" p", Action::Palette),
    ] {
        let mut result = None;
        for c in keys.chars() {
            result = map.resolve(
                Mode::Normal,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        assert_eq!(result, Some(expected));
        assert!(map.pending().is_empty());
    }
    let config = Config::parse(
        "[keybindings.normal]\n'space z'='find_files'\n[keybindings.picker]\n'ctrl-j'='down'",
    )
    .unwrap();
    let mut map = Keymap::new(&config).unwrap();
    assert_eq!(
        map.resolve(
            Mode::Picker,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)
        ),
        Some(Action::Down)
    );
    for text in [
        "[visual]\nfps=0",
        "[visual]\ntrail_intensity=11",
        "[visual]\ncursor_duration_ms=0",
        "[navigation]\nmax_results=0",
        "[navigation]\nexplorer_width=2",
    ] {
        assert!(Config::parse(text).is_err());
    }
}
#[test]
fn asynchronous_open_keeps_unsaved_buffers_and_picker_accepts_results() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let mut app = app(&root);
    app.act(Action::Insert);
    app.type_text("unsaved 日本");
    app.act(Action::Normal);
    app.command("e main.rs");
    app.act(Action::Insert);
    app.type_text("!");
    app.act(Action::Normal);
    wait(&mut app, |a| a.workspace.buffers.len() == 2);
    assert!(
        app.workspace.buffers[&1]
            .buffer
            .text
            .to_string()
            .ends_with('!')
    );
    app.open_picker(PickerKind::Files);
    app.type_text("main");
    wait(&mut app, |a| {
        a.picker
            .as_ref()
            .is_some_and(|p| !p.busy && p.results.len() == 1)
    });
    assert_eq!(
        app.picker.as_ref().unwrap().results[0].item.label,
        "main.rs"
    );
    app.act(Action::Submit);
    assert!(app.picker.is_none());
    app.command("qa");
    assert!(!app.exit_requested);
    app.command("qa!");
    wait(&mut app, App::ready_to_exit);
}
#[test]
fn explorer_operations_require_confirmation_and_update_open_buffers() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::write(root.join("a.txt"), "hello").unwrap();
    let mut app = app(&root);
    app.command("e a.txt");
    wait(&mut app, |a| a.workspace.buffers.len() == 2 && !a.indexing);
    app.act(Action::Explorer);
    wait(&mut app, |a| {
        !a.explorer.busy && !a.explorer.rows.is_empty()
    });
    app.explorer.selected = app
        .explorer
        .rows
        .iter()
        .position(|r| r.entry.path == root.join("a.txt"))
        .unwrap();
    app.act(Action::RenameFile);
    app.dialog.as_mut().unwrap().input = "renamed.txt".into();
    app.act(Action::Submit);
    wait(&mut app, |a| !a.mutation_pending && !a.indexing);
    assert!(root.join("renamed.txt").exists());
    assert_eq!(
        app.workspace.editor().buffer.path.as_deref(),
        Some(root.join("renamed.txt").as_path())
    );
    wait(&mut app, |a| !a.explorer.busy);
    app.explorer.selected = app
        .explorer
        .rows
        .iter()
        .position(|r| r.entry.path == root.join("renamed.txt"))
        .unwrap();
    app.act(Action::DeleteFile);
    assert!(root.join("renamed.txt").exists());
    app.type_text("delete");
    app.act(Action::Submit);
    wait(&mut app, |a| !a.mutation_pending);
    assert!(!root.join("renamed.txt").exists());
    assert!(!app.workspace.buffers.values().any(|e| {
        e.buffer
            .path
            .as_ref()
            .is_some_and(|p| p.ends_with("renamed.txt"))
    }));
    app.command("qa!");
    wait(&mut app, App::ready_to_exit);
}
#[test]
fn workspace_ui_handles_small_resizes_dialogs_picker_and_shared_splits() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    let mut app = app(&root);
    app.act(Action::Insert);
    app.type_text("日本 👩‍💻\nhello\n");
    app.act(Action::Normal);
    app.act(Action::SplitVertical);
    app.act(Action::SplitHorizontal);
    let mut visuals = Visuals::new(app.workspace.config.effects());
    for (width, height) in [(110, 34), (65, 18), (28, 10), (12, 6), (2, 1)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                workspace_ui::draw(
                    f,
                    &mut app,
                    &Theme::default(),
                    &BTreeMap::new(),
                    &mut visuals,
                    "",
                    Instant::now(),
                )
            })
            .unwrap();
        app.open_picker(PickerKind::Palette);
        wait(&mut app, |a| a.picker.as_ref().is_some_and(|p| !p.busy));
        terminal
            .draw(|f| {
                workspace_ui::draw(
                    f,
                    &mut app,
                    &Theme::default(),
                    &BTreeMap::new(),
                    &mut visuals,
                    "",
                    Instant::now(),
                )
            })
            .unwrap();
        app.act(Action::Normal);
        app.command("cd");
        terminal
            .draw(|f| {
                workspace_ui::draw(
                    f,
                    &mut app,
                    &Theme::default(),
                    &BTreeMap::new(),
                    &mut visuals,
                    "",
                    Instant::now(),
                )
            })
            .unwrap();
        app.act(Action::Normal);
    }
    assert!(
        app.workspace
            .views
            .values()
            .all(|v| v.buffer == app.workspace.active_buffer())
    );
    app.command("qa!");
    wait(&mut app, App::ready_to_exit);
}

#[test]
fn saving_a_snapshot_does_not_mark_newer_edits_clean_or_quit() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    let mut app = app(&root);
    app.act(Action::Insert);
    app.type_text("saved 日本");
    app.act(Action::Normal);
    app.command("wq saved.txt");
    app.act(Action::Insert);
    app.type_text(" newer");
    app.act(Action::Normal);
    wait(&mut app, |a| a.saves.is_empty());
    assert!(!app.exit_requested);
    assert!(app.workspace.editor().buffer.dirty());
    assert_eq!(
        fs::read_to_string(root.join("saved.txt")).unwrap(),
        "saved 日本"
    );
    app.command("wq");
    wait(&mut app, App::ready_to_exit);
    assert_eq!(
        fs::read_to_string(root.join("saved.txt")).unwrap(),
        "saved 日本 newer"
    );
}
#[test]
fn modified_buffer_close_is_explicit_and_other_buffers_survive() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    let mut app = app(&root);
    app.act(Action::Insert);
    app.type_text("first");
    app.act(Action::Normal);
    app.act(Action::New);
    app.act(Action::Insert);
    app.type_text("second");
    app.act(Action::Normal);
    let second = app.workspace.active_buffer();
    app.act(Action::CloseBuffer);
    assert!(app.dialog.is_some());
    app.act(Action::Normal);
    assert!(app.workspace.buffers.contains_key(&second));
    app.act(Action::CloseBuffer);
    app.type_text("discard");
    app.act(Action::Submit);
    assert!(!app.workspace.buffers.contains_key(&second));
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "first");
    app.command("qa");
    assert!(!app.exit_requested);
    app.command("new!");
    assert!(!app.workspace.dirty());
    app.command("qa");
    wait(&mut app, App::ready_to_exit);
}
#[test]
fn root_switch_and_recent_projects_preserve_open_documents() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir(root.join("other")).unwrap();
    fs::write(root.join("other/new.rs"), "hello").unwrap();
    let mut app = app(&root);
    app.act(Action::Insert);
    app.type_text("keep me");
    app.act(Action::Normal);
    app.command("cd other");
    wait(&mut app, |a| {
        a.workspace.root.ends_with("other") && !a.indexing
    });
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "keep me");
    assert!(app.workspace.editor().buffer.dirty());
    assert_eq!(app.history.projects[0], root.join("other"));
    assert!(app.index.files.iter().any(|i| i.label == "new.rs"));
    app.open_picker(PickerKind::Projects);
    wait(&mut app, |a| a.picker.as_ref().is_some_and(|p| !p.busy));
    assert_eq!(app.picker.as_ref().unwrap().results.len(), 2);
    app.act(Action::Normal);
    app.command("qa!");
    wait(&mut app, App::ready_to_exit);
}
#[test]
fn stale_opens_and_queries_do_not_replace_latest_choices() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::write(root.join("a.rs"), "a").unwrap();
    fs::write(root.join("b.rs"), "b").unwrap();
    let mut app = app(&root);
    app.command("e a.rs");
    app.command("e b.rs");
    wait(&mut app, |a| {
        a.workspace
            .editor()
            .buffer
            .path
            .as_ref()
            .is_some_and(|p| p.ends_with("b.rs"))
    });
    assert!(
        !app.workspace.buffers.values().any(|e| e
            .buffer
            .path
            .as_ref()
            .is_some_and(|p| p.ends_with("a.rs")))
    );
    app.open_picker(PickerKind::Files);
    app.type_text("a");
    app.act(Action::Backspace);
    app.type_text("b");
    wait(&mut app, |a| a.picker.as_ref().is_some_and(|p| !p.busy));
    assert_eq!(app.picker.as_ref().unwrap().results[0].item.label, "b.rs");
    app.act(Action::Normal);
    app.command("qa");
    wait(&mut app, App::ready_to_exit);
}
#[test]
fn edits_made_during_confirmed_deletion_are_retained_and_saves_wait() {
    let temp = root();
    let root = fs::canonicalize(temp.path()).unwrap();
    let path = root.join("a.txt");
    fs::write(&path, "original").unwrap();
    let mut app = app(&root);
    app.command("e a.txt");
    wait(&mut app, |a| a.workspace.buffers.len() == 2 && !a.indexing);
    let id = app.workspace.active_buffer();
    app.act(Action::Explorer);
    wait(&mut app, |a| !a.explorer.busy);
    app.explorer.selected = app
        .explorer
        .rows
        .iter()
        .position(|r| r.entry.path == path)
        .unwrap();
    app.act(Action::DeleteFile);
    app.type_text("delete");
    app.act(Action::Submit);
    assert!(app.mutation_pending);
    app.explorer.focused = false;
    app.act(Action::Insert);
    app.type_text("newer ");
    app.act(Action::Normal);
    app.act(Action::Save);
    assert!(app.saves.is_empty());
    wait(&mut app, |a| !a.mutation_pending);
    assert!(!path.exists());
    assert!(app.workspace.buffers.contains_key(&id));
    assert!(app.workspace.buffers[&id].buffer.path.is_none());
    assert!(app.workspace.buffers[&id].buffer.dirty());
    assert_eq!(
        app.workspace.buffers[&id].buffer.text.to_string(),
        "newer original"
    );
    app.command("qa!");
    wait(&mut app, App::ready_to_exit);
}
