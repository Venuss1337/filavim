use filavim::{
    app::App,
    buffer::Buffer,
    config::Config,
    editor::{Editor, Mode},
    git,
    ide::{PanelKind, PromptAction},
    input::{Action, Keymap},
    lsp::{self, Client, Encoding, Event},
    setup,
    storage::History,
    terminal,
    workspace::Workspace,
};
use ropey::Rope;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{OnceLock, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};
fn mock() -> PathBuf {
    static MOCK: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    MOCK.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(if cfg!(windows) {
            "mock-lsp.exe"
        } else {
            "mock-lsp"
        });
        let mut command = Command::new("rustc");
        command
            .args(["--edition=2024", "tests/fixtures/mock_lsp.rs", "-o"])
            .arg(&path);
        setup::hide_window(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (dir, path)
    })
    .1
    .clone()
}
fn server(log: &Path, flags: &[&str]) -> lsp::ServerConfig {
    lsp::ServerConfig {
        name: "mock".into(),
        command: mock().to_string_lossy().into(),
        args: std::iter::once(log.to_string_lossy().into_owned())
            .chain(flags.iter().map(|s| s.to_string()))
            .collect(),
        languages: vec!["rust".into()],
        ..Default::default()
    }
}
fn until(mut test: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if test() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not complete before timeout")
}
fn ready(client: &Client) {
    until(|| {
        client.events.try_iter().any(|e| {
            matches!(
                e,
                Event::Ready {
                    encoding: Encoding::Utf16,
                    ..
                }
            )
        })
    });
}
fn app(root: &Path, text: &str, server: Option<lsp::ServerConfig>) -> App {
    let mut config = Config::default();
    config.git.enabled = false;
    config.lsp.enabled = server.is_some();
    if let Some(server) = server {
        config.lsp.servers = vec![server];
    }
    let mut e = Editor::new(config);
    let path = root.join("main.rs");
    std::fs::write(&path, text).unwrap();
    e.set_buffer(Buffer::new(Rope::from_str(text), Some(path)));
    App::with_history_path(
        Workspace::new(e, root.into()),
        History::default(),
        root.join("history.toml"),
    )
}
fn pump(app: &mut App, condition: impl Fn(&App) -> bool) {
    until(|| {
        app.poll();
        condition(app)
    })
}
#[test]
fn protocol_initialize_sync_requests_diagnostics_cancel_and_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire.log");
    let client = Client::start(server(&log, &[]), dir.path().into(), Duration::from_secs(2));
    ready(&client);
    let uri = lsp::uri(&dir.path().join("main.rs")).unwrap();
    client.document(
        uri.clone(),
        "rust".into(),
        0,
        Rope::from_str("a😀\r\n"),
        true,
    );
    let mut diagnostic = false;
    until(|| {
        for e in client.events.try_iter() {
            if let Event::Notification { method, params } = e
                && method == "textDocument/publishDiagnostics"
            {
                assert_eq!(params["uri"], uri);
                diagnostic = true;
            }
        }
        diagnostic
    });
    client.document(uri, "rust".into(), 1, Rope::from_str("changed"), false);
    client.request(1, "echo", json!({"value":1}));
    until(|| {
        client.events.try_iter().any(|e| {
            matches!(
                e,
                Event::Reply {
                    id: 1,
                    error: None,
                    ..
                }
            )
        })
    });
    client.request(2, "slow", json!({}));
    client.cancel(2);
    client.request(3, "echo", json!({}));
    let mut echoed = false;
    until(|| {
        for e in client.events.try_iter() {
            match e {
                Event::Reply { id: 2, .. } => panic!("cancelled reply leaked"),
                Event::Reply { id: 3, .. } => echoed = true,
                _ => {}
            }
        }
        echoed
    });
    client.request(4, "askApply", json!({}));
    until(|| {
        let mut applied = false;
        for e in client.events.try_iter() {
            if let Event::ServerRequest { id, method, .. } = e {
                assert_eq!(method, "workspace/applyEdit");
                client.respond(id, json!({"applied":false}));
                applied = true;
            }
        }
        applied
    });
    client.shutdown();
    until(|| client.stopped.load(Ordering::Acquire));
    let log = std::fs::read_to_string(log).unwrap();
    for method in [
        "initialized",
        "didOpen",
        "didChange",
        "cancelRequest",
        "shutdown",
        "exit",
    ] {
        assert!(log.contains(method), "{method}");
    }
    assert!(log.contains("\"result\":[null,null]"));
}
#[test]
fn protocol_timeouts_crashes_and_malformed_messages_are_graceful() {
    for flag in ["--noinit", "--crash", "--malformed"] {
        let dir = tempfile::tempdir().unwrap();
        let c = Client::start(
            server(&dir.path().join("log"), &[flag]),
            dir.path().into(),
            Duration::from_millis(120),
        );
        until(|| c.events.try_iter().any(|e| matches!(e, Event::Stopped(_))));
        until(|| c.stopped.load(Ordering::Acquire));
    }
    let dir = tempfile::tempdir().unwrap();
    let c = Client::start(
        server(&dir.path().join("log"), &[]),
        dir.path().into(),
        Duration::from_millis(150),
    );
    ready(&c);
    c.request(8, "slow", json!({}));
    until(|| {
        c.events.try_iter().any(|e| {
            matches!(
                e,
                Event::Reply {
                    id: 8,
                    error: Some(_),
                    ..
                }
            )
        })
    });
}

#[test]
fn incremental_utf8_sync_and_forced_cleanup_of_a_stuck_server() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("incremental");
    let c = Client::start(
        server(&log, &["--incremental", "--utf8"]),
        dir.path().into(),
        Duration::from_secs(2),
    );
    until(|| {
        c.events.try_iter().any(|e| {
            matches!(
                e,
                Event::Ready {
                    encoding: Encoding::Utf8,
                    ..
                }
            )
        })
    });
    let uri = lsp::uri(&dir.path().join("main.rs")).unwrap();
    c.document(uri.clone(), "rust".into(), 0, Rope::from_str("a😀"), true);
    c.document(uri, "rust".into(), 1, Rope::from_str("next"), false);
    c.request(10, "echo", json!({}));
    until(|| {
        c.events
            .try_iter()
            .any(|e| matches!(e, Event::Reply { id: 10, .. }))
    });
    c.shutdown();
    until(|| c.stopped.load(Ordering::Acquire));
    let lines = std::fs::read_to_string(log).unwrap();
    let messages = lines
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .collect::<Vec<_>>();
    let change = messages
        .iter()
        .find(|v| v["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(
        change["params"]["contentChanges"][0]["range"]["end"]["character"],
        5
    );
    assert_eq!(change["params"]["textDocument"]["version"], 1);
    let c = Client::start(
        server(&dir.path().join("stuck"), &["--stuck"]),
        dir.path().into(),
        Duration::from_secs(2),
    );
    ready(&c);
    c.document(
        lsp::uri(&dir.path().join("main.rs")).unwrap(),
        "rust".into(),
        0,
        Rope::from_str(&"x".repeat(2_000_000)),
        true,
    );
    c.shutdown();
    until(|| c.stopped.load(Ordering::Acquire));
}

#[test]
fn stale_completion_is_discarded_before_it_can_edit_the_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire");
    let mut a = app(dir.path(), "va", Some(server(&log, &[])));
    until(|| {
        a.poll();
        a.command("LspInfo");
        let r = a
            .ide
            .panel
            .as_ref()
            .is_some_and(|p| p.text.contains("Ready"));
        a.ide.panel = None;
        r
    });
    a.workspace.editor_mut().buffer.cursor = 2;
    a.workspace.stash();
    a.act(Action::Insert);
    a.act(Action::Complete);
    a.type_text("x");
    a.poll();
    assert_eq!(a.workspace.editor().buffer.text.to_string(), "vax");
    assert!(
        a.ide
            .panel
            .as_ref()
            .is_none_or(|p| p.kind != PanelKind::Completion)
    );
    a.ide.shutdown();
    pump(&mut a, |a| a.ide.stopped());
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains("cancelRequest")
    );
}

#[test]
fn json_rpc_framing_counts_utf8_bytes_and_rejects_malformed_or_oversized_input() {
    let message = json!({"jsonrpc":"2.0","method":"test","params":"日本 😀"});
    let mut bytes = Vec::new();
    lsp::write_message(&mut bytes, &message).unwrap();
    assert_eq!(
        lsp::read_message(&mut std::io::Cursor::new(bytes)).unwrap(),
        message
    );
    for data in [
        "Content-Length: 16777217\r\n\r\n",
        "Wrong: 2\r\n\r\n{}",
        "Content-Length: 4\r\n\r\nnope",
    ] {
        assert!(lsp::read_message(&mut std::io::Cursor::new(data.as_bytes())).is_err());
    }
}

#[test]
fn ide_panels_and_diagnostic_markers_render_headlessly_at_small_sizes() {
    use filavim::{animation::Visuals, ide::Diagnostic, theme::Theme, workspace_ui};
    use ratatui::{Terminal, backend::TestBackend};
    let dir = tempfile::tempdir().unwrap();
    let mut a = app(dir.path(), "let value = 1;\n", None);
    let path = a.workspace.editor().buffer.path.clone().unwrap();
    a.ide.diagnostics.insert(
        lsp::path_key(&path),
        vec![Diagnostic {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 4),
                lsp_types::Position::new(0, 9),
            ),
            severity: 1,
            message: "Headless error".into(),
            encoding: Encoding::Utf16,
            raw: json!({}),
            revision: 0,
        }],
    );
    for size in [(100, 30), (35, 12), (12, 6), (8, 3)] {
        for command in ["Settings", "Setup", "LspInfo"] {
            a.ide.panel = None;
            a.ide.prompt = None;
            a.command(command);
            let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            let mut visuals = Visuals::new(a.workspace.config.effects());
            terminal
                .draw(|f| {
                    workspace_ui::draw(
                        f,
                        &mut a,
                        &Theme::default(),
                        &BTreeMap::new(),
                        &mut visuals,
                        "",
                        Instant::now(),
                    )
                })
                .unwrap();
            assert!(!visuals.active(Instant::now()));
        }
    }
    a.ide.panel = None;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut visuals = Visuals::new(a.workspace.config.effects());
    terminal
        .draw(|f| {
            workspace_ui::draw(
                f,
                &mut a,
                &Theme::default(),
                &BTreeMap::new(),
                &mut visuals,
                "",
                Instant::now(),
            )
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(1, 3)].symbol(), "●");
}
#[test]
fn utf_positions_roundtrip_and_reject_split_surrogates() {
    let text = Rope::from_str("a😀e\u{301}\r\n日本\n");
    for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
        for index in [0, 1, 2, 3, 4, 6, 7, 8, 9] {
            let p = lsp::position(&text, index, encoding);
            assert_eq!(lsp::offset(&text, p, encoding).unwrap(), index);
        }
    }
    assert!(lsp::offset(&text, lsp_types::Position::new(0, 2), Encoding::Utf16).is_err());
    assert!(lsp::offset(&text, lsp_types::Position::new(0, 2), Encoding::Utf8).is_err());
    assert!(lsp::offset(&text, lsp_types::Position::new(99, 0), Encoding::Utf16).is_err());
}
#[test]
fn workspace_edit_is_transactional_versioned_and_undoable_across_buffers() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "abc😀\n", None);
    let main = dir.path().join("main.rs");
    let other = dir.path().join("other.rs");
    std::fs::write(&other, "xyz\n").unwrap();
    let snapshot = BTreeMap::from([(
        lsp::path_key(&main),
        lsp::Snapshot {
            text: app.workspace.editor().buffer.text.clone(),
            revision: 0,
        },
    )]);
    let edit = json!({"changes":{lsp::uri(&main).unwrap():[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"A"}],lsp::uri(&other).unwrap():[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"newText":"Y"}]}});
    let plan = lsp::plan_edit(&edit, &snapshot, Encoding::Utf16).unwrap();
    filavim::ide::apply_plan(&mut app, plan).unwrap();
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "Abc😀\n");
    let id = app.workspace.find_path(&other).unwrap();
    assert_eq!(app.workspace.buffers[&id].buffer.text.to_string(), "xYz\n");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "xyz\n");
    app.act(Action::Undo);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "abc😀\n");
    let plan = lsp::plan_edit(&edit, &snapshot, Encoding::Utf16).unwrap();
    assert!(filavim::ide::apply_plan(&mut app, plan).is_err());
    assert_eq!(app.workspace.buffers[&id].buffer.text.to_string(), "xYz\n");
    let invalid = json!({"documentChanges":[{"kind":"delete","uri":lsp::uri(&other).unwrap()}]});
    assert!(lsp::plan_edit(&invalid, &snapshot, Encoding::Utf16).is_err());
}
#[test]
fn completion_snippet_format_rename_and_code_action_use_real_mock_transport() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire");
    let mut app = app(dir.path(), "va", Some(server(&log, &[])));
    app.poll();
    until(|| {
        app.poll();
        app.command("LspInfo");
        let ready = app
            .ide
            .panel
            .as_ref()
            .is_some_and(|p| p.text.contains("Ready"));
        app.ide.panel = None;
        ready
    });
    app.workspace.editor_mut().buffer.cursor = 2;
    app.workspace.stash();
    app.act(Action::Insert);
    app.act(Action::Complete);
    pump(&mut app, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Completion)
    });
    app.act(Action::Tab);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "variable");
    app.type_text("name");
    app.act(Action::Tab);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "name");
    assert_eq!(app.workspace.editor().buffer.cursor, 4);
    app.act(Action::Normal);
    app.command("Format");
    pump(&mut app, |a| a.ide.prompt.is_some());
    assert!(matches!(
        app.ide.prompt.as_ref().unwrap().action,
        PromptAction::Apply(_)
    ));
    app.type_text("apply");
    app.act(Action::Submit);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "Fame");
    app.command("Rename Z");
    pump(&mut app, |a| a.ide.prompt.is_some());
    app.type_text("apply");
    app.act(Action::Submit);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "Zame");
    app.command("CodeActions");
    pump(&mut app, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Actions)
    });
    app.act(Action::Submit);
    pump(&mut app, |a| a.ide.prompt.is_some());
    app.type_text("apply");
    app.act(Action::Submit);
    assert_eq!(app.workspace.editor().buffer.text.to_string(), "Qame");
    app.command("Hover");
    pump(&mut app, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.text.contains("Mock hover"))
    });
    app.ide.shutdown();
    pump(&mut app, |a| a.ide.stopped());
}
#[test]
fn automatic_signature_help_tracks_arguments_and_hides_without_stealing_insert_focus() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire");
    let mut app = app(dir.path(), "f", Some(server(&log, &[])));
    pump(&mut app, |a| a.ide.message.is_empty());
    app.workspace.editor_mut().buffer.cursor = 1;
    app.workspace.stash();
    app.act(Action::Insert);
    app.type_text("(");
    pump(&mut app, |a| a.ide.signature.is_some());
    assert_eq!(app.mode(), Mode::Insert);
    let label =
        filavim::contextual::signature_label(&app.ide.signature.as_ref().unwrap().help).unwrap();
    assert_eq!(&label.label[label.active.unwrap()], "first: i32");
    app.type_text("1,");
    pump(&mut app, |a| {
        a.ide
            .signature
            .as_ref()
            .is_some_and(|s| s.help.active_parameter == Some(1))
    });
    let label =
        filavim::contextual::signature_label(&app.ide.signature.as_ref().unwrap().help).unwrap();
    assert_eq!(&label.label[label.active.unwrap()], "second: str");
    app.type_text("2)");
    app.poll();
    assert!(app.ide.signature.is_none());
    assert_eq!(app.mode(), Mode::Insert);
    app.act(Action::Normal);
    app.poll();
    assert!(app.ide.signature.is_none());
    let messages: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m["method"] == "textDocument/signatureHelp"
                && m["params"]["context"]["triggerCharacter"] == ",")
    );
    app.ide.shutdown();
    pump(&mut app, |a| a.ide.stopped());
}

#[test]
fn completion_refresh_keeps_popup_and_enter_applies_only_a_current_response() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire");
    let mut app = app(dir.path(), "v", Some(server(&log, &[])));
    app.workspace.editor_mut().buffer.cursor = 1;
    app.workspace.stash();
    app.act(Action::Insert);
    app.act(Action::Complete);
    pump(&mut app, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Completion)
    });
    app.ide.panel.as_mut().unwrap().popup_size = (31, 8);
    app.type_text("a");
    app.poll();
    let panel = app
        .ide
        .panel
        .as_ref()
        .expect("cached menu should survive typing");
    assert_eq!(panel.query, "va");
    assert_eq!(panel.popup_size, (31, 8));
    let mut map = Keymap::new(&Config::default()).unwrap();
    assert_eq!(
        map.resolve(
            Mode::Insert,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)
        ),
        Some(Action::Down)
    );
    assert_eq!(
        map.resolve(
            Mode::Insert,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
        ),
        Some(Action::Up)
    );
    let enter = map
        .resolve(
            Mode::Insert,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
    let request_count = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|line| line.contains("textDocument/completion"))
        .count();
    app.act(enter);
    assert_eq!(
        app.workspace.editor().buffer.text,
        "variable",
        "safe cached snippet accepts synchronously"
    );
    assert_eq!(
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|line| line.contains("textDocument/completion"))
            .count(),
        request_count
    );
    pump(&mut app, |a| a.workspace.editor().buffer.text == "variable");
    assert!(
        !app.workspace
            .editor()
            .buffer
            .text
            .to_string()
            .contains('\n')
    );
    app.type_text("v");
    app.command("Settings");
    let began = Instant::now();
    until(|| {
        app.poll();
        began.elapsed() > Duration::from_millis(180)
    });
    assert!(
        app.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Settings),
        "completion must not steal focus from settings"
    );
    app.act(Action::Normal);
    app.poll();
    assert!(app.ide.panel.is_none());
    app.ide.shutdown();
    pump(&mut app, |a| a.ide.stopped());
}

#[test]
fn native_snippets_and_invalid_edits_are_checked() {
    let ex = filavim::snippet::expand("fn ${1:name}(${2:arg}) { $0 }").unwrap();
    assert_eq!(ex.text, "fn name(arg) {  }");
    assert_eq!(ex.fields[0], 3..7);
    let ex = filavim::snippet::normalize(
        filavim::snippet::expand("first\n${1:é}\n$0").unwrap(),
        "\r\n",
    );
    assert_eq!(ex.text, "first\r\né\r\n");
    assert_eq!(ex.fields, vec![7..8, 10..10]);
    assert!(filavim::snippet::expand("${TM_FILENAME}").is_err());
    let text = Rope::from_str("abc");
    let edit = lsp_types::TextEdit {
        range: lsp_types::Range::new(
            lsp_types::Position::new(0, 0),
            lsp_types::Position::new(0, 2),
        ),
        new_text: "x".into(),
    };
    assert!(lsp::ranges(&text, &[edit.clone(), edit], Encoding::Utf16).is_err());
}
#[test]
fn settings_roundtrip_preserves_cursor_and_rejects_invalid_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,"[visual]\nfps=60\ntrail_intensity=10\n[visual.smear]\ntrailing_stiffness=0.32\ngradient_exponent=0.7\n").unwrap();
    let c = setup::save_setting(&path, "line_numbers", "false").unwrap();
    assert!(!c.line_numbers);
    assert_eq!(c.effects().fps, 60);
    assert_eq!(c.visual.smear.trailing_stiffness, 0.32);
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(setup::save_setting(&path, "visual.fps", "0").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    let c = setup::save_setting(&path, "server.rust.command", "custom-rust-analyzer").unwrap();
    assert_eq!(c.lsp.servers[0].command, "custom-rust-analyzer");
    let c = setup::save_setting(&path, "server.rust.enabled", "false").unwrap();
    assert!(!c.lsp.servers[0].enabled);
    assert_eq!(
        Config::parse(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .visual
            .smear
            .gradient_exponent,
        0.7
    );
}
#[test]
fn setup_requires_confirmation_and_ide_key_sequences_do_not_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path(), "", None);
    app.command("LspInstall rust");
    assert!(matches!(
        app.ide.prompt.as_ref().unwrap().action,
        PromptAction::Install(_)
    ));
    assert_eq!(app.ide.busy, 0);
    app.act(Action::Normal);
    assert_eq!(app.ide.busy, 0);
    let mut map = Keymap::new(&Config::default()).unwrap();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for (sequence, action) in [
        (" ca", Action::CodeActions),
        (" cr", Action::RenameSymbol),
        (" cf", Action::FormatDocument),
        (" gd", Action::Definition),
        (" gr", Action::References),
        (" gs", Action::GitStatus),
        (" gh", Action::GitDiff),
        (" t", Action::Terminal),
        (" l", Action::LspInfo),
        (" btn", Action::NewTab),
    ] {
        map.clear();
        for (i, c) in sequence.chars().enumerate() {
            let resolved = map.resolve(
                Mode::Normal,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
            if i == sequence.len() - 1 {
                assert_eq!(resolved, Some(action), "{sequence}")
            } else {
                assert_eq!(resolved, Some(Action::Noop), "{sequence}")
            }
        }
    }
}
#[test]
fn git_status_diff_staging_hunks_unstage_and_previous_work_in_a_temp_repo() {
    if setup::executable("git").is_none() {
        eprintln!("Git unavailable; repository test skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let run = |args: &[&str]| {
        let mut c = Command::new("git");
        c.current_dir(root).args(args);
        setup::run(c, Duration::from_secs(15)).unwrap()
    };
    run(&["init"]);
    run(&["config", "user.name", "Filavim Test"]);
    run(&["config", "user.email", "test@example.invalid"]);
    let path = root.join("file.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    run(&["add", "file.txt"]);
    run(&["commit", "-m", "initial"]);
    std::fs::write(&path, "one\nTWO\nthree\n").unwrap();
    let state = git::inspect(root, Some(&path), None).unwrap();
    assert!(!state.branch.is_empty());
    assert!(!state.lines.is_empty());
    let diff = git::execute(git::Job::Diff {
        root: root.into(),
        path: path.clone(),
        staged: false,
    })
    .unwrap();
    let git::Event::Diff(state) = diff else {
        panic!()
    };
    assert_eq!(state.hunks.len(), 1);
    git::apply_hunk(root, &state.hunks[0].patch, false).unwrap();
    assert!(run(&["diff", "--cached"]).contains("+TWO"));
    git::apply_hunk(root, &state.hunks[0].patch, true).unwrap();
    assert!(run(&["diff", "--cached"]).trim().is_empty());
    git::execute(git::Job::Stage {
        root: root.into(),
        path: path.clone(),
        undo: false,
    })
    .unwrap();
    assert!(run(&["diff", "--cached"]).contains("+TWO"));
    git::execute(git::Job::Stage {
        root: root.into(),
        path: path.clone(),
        undo: true,
    })
    .unwrap();
    assert!(run(&["diff", "--cached"]).trim().is_empty());
    let git::Event::Previous { text, .. } = git::execute(git::Job::Previous {
        root: root.into(),
        path,
    })
    .unwrap() else {
        panic!()
    };
    assert_eq!(text, "one\ntwo\nthree\n");
    run(&["add", "file.txt"]);
    git::execute(git::Job::Commit {
        root: root.into(),
        message: "confirmed test commit".into(),
    })
    .unwrap();
    assert_eq!(
        run(&["log", "-1", "--format=%s"]).trim(),
        "confirmed test commit"
    );
}
#[test]
fn noninteractive_pty_process_output_resize_and_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let config = terminal::Config {
        shell: if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into(),
        args: if cfg!(windows) {
            vec!["/d".into(), "/c".into(), "echo FILAVIM_PTY_TEST".into()]
        } else {
            vec![
                "-c".into(),
                "printf '\x1b[31mFILAVIM_PTY_TEST\x1b[0m\\n'".into(),
            ]
        },
        ..Default::default()
    };
    let mut session = terminal::Session::start(config, dir.path().into(), 12, 80);
    session.resize(18, 90);
    until(|| session.ended.load(Ordering::Acquire));
    session.poll();
    assert!(
        session.error.lock().unwrap().is_none(),
        "{:?}",
        session.error.lock().unwrap()
    );
    assert!(session.display.contents().contains("FILAVIM_PTY_TEST"));
    let config = terminal::Config {
        shell: if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into(),
        args: Vec::new(),
        ..Default::default()
    };
    let mut session = terminal::Session::start(config, dir.path().into(), 12, 80);
    session.resize(20, 100);
    until(|| {
        session.poll();
        session.display.size().0.get() == 20
    });
    session.close();
    until(|| session.ended.load(Ordering::Acquire));
}

#[test]
fn hidden_terminal_parses_modes_without_snapshots_and_refreshes_when_shown() {
    let dir = tempfile::tempdir().unwrap();
    let session = terminal::Session::start(
        terminal::Config {
            shell: mock().to_string_lossy().into(),
            args: vec!["--terminal-output".into()],
            ..Default::default()
        },
        dir.path().into(),
        12,
        80,
    );
    session.set_visible(false);
    thread::sleep(Duration::from_millis(100));
    let hidden = session.publications.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(session.publications.load(Ordering::Acquire), hidden);
    // ConPTY may consume DEC input-mode sequences. Their snapshot-independent
    // publication is tested directly against the VT100 parser in the unit test.
    session.set_visible(true);
    until(|| session.publications.load(Ordering::Acquire) > hidden);
    session.set_visible(false);
    thread::sleep(Duration::from_millis(40));
    let hidden = session.publications.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(60));
    assert_eq!(session.publications.load(Ordering::Acquire), hidden);
    session.close();
    until(|| session.ended.load(Ordering::Acquire));
}

#[test]
fn large_lsp_results_two_servers_and_editor_input_preserve_identities() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = server(&dir.path().join("alpha"), &["--flood", "--incremental"]);
    first.name = "alpha".into();
    let mut second = server(&dir.path().join("beta"), &["--flood", "--incremental"]);
    second.name = "beta".into();
    second.languages = vec!["python".into()];
    let mut a = app(dir.path(), "fn main() {}\n", Some(first.clone()));
    a.workspace.config.lsp.servers = vec![first, second];
    let path = dir.path().join("other.py");
    std::fs::write(&path, "value = 1\n").unwrap();
    a.workspace.insert_buffer(Buffer::new(
        Rope::from_str("value = 1\n"),
        Some(path.clone()),
    ));
    a.act(Action::Insert);
    for _ in 0..120 {
        a.poll();
        a.type_text("z");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        a.workspace
            .editor()
            .buffer
            .text
            .to_string()
            .starts_with(&"z".repeat(120))
    );
    // Another server must still publish its unaffected document.
    pump(&mut a, |a| {
        a.ide
            .diagnostics
            .get(&lsp::path_key(&path))
            .is_some_and(|d| d.len() == 1)
    });
    a.act(Action::Normal);
    a.act(Action::References);
    a.act(Action::Right); // Navigation alone does not invalidate reference results.
    pump(&mut a, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Locations && p.rows.len() == 20_000)
    });
    a.act(Action::Normal);
    a.act(Action::CodeActions);
    pump(&mut a, |a| {
        a.ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Actions && p.rows.len() == 20_000)
    });
    a.act(Action::Normal);
    // A stale response arriving after a mutation must not install its panel.
    a.act(Action::References);
    a.act(Action::Insert);
    a.type_text("x");
    for _ in 0..40 {
        a.poll();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        a.ide
            .panel
            .as_ref()
            .is_none_or(|p| p.kind != PanelKind::Locations)
    );
    a.ide.shutdown();
    until(|| a.ide.stopped());
}

#[test]
fn blocked_lsp_write_times_out_without_explicit_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let c = Client::start(
        server(&dir.path().join("blocked"), &["--stuck"]),
        dir.path().into(),
        Duration::from_millis(250),
    );
    ready(&c);
    c.document(
        lsp::uri(&dir.path().join("main.rs")).unwrap(),
        "rust".into(),
        0,
        Rope::from_str(&"x".repeat(2_000_000)),
        true,
    );
    until(|| c.stopped.load(Ordering::Acquire));
}
