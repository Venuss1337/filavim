mod support;
#[path = "support/workspace_resources.rs"]
mod workspace_resources;
use filavim::{
    buffer::Buffer,
    config::Config,
    gui::{
        documents::Documents,
        io::{self, Done},
        model::Model,
        services::{Kind, Services},
    },
    input::Action,
    lsp,
};
use ropey::Rope;
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
fn docs() -> Documents {
    let mut config = Config::default();
    config.lsp.enabled = false;
    Documents::new(Model::new(config).unwrap())
}
fn add(d: &mut Documents, path: &Path, text: &str) -> u64 {
    d.add(
        Buffer::new(Rope::from_str(text), Some(path.to_owned())),
        true,
    )
    .unwrap()
}
#[test]
fn document_undo_selection_and_viewport_are_independent() {
    let mut d = docs();
    let a = add(&mut d, Path::new("a.rs"), "one");
    d.editor.buffer.cursor = 3;
    d.editor.buffer.insert("α");
    d.editor.top = 12;
    d.editor.anchor = Some(1);
    let b = add(&mut d, Path::new("b.rs"), "two");
    d.editor.buffer.insert("😀");
    d.act(Action::Undo);
    assert_eq!(d.editor.buffer.text.to_string(), "two");
    d.act(Action::Redo);
    assert_eq!(d.editor.buffer.text.to_string(), "😀two");
    d.switch(a);
    assert_eq!(d.editor.buffer.text.to_string(), "oneα");
    assert_eq!(d.editor.top, 12);
    assert_eq!(d.editor.anchor, Some(1));
    d.act(Action::Undo);
    assert_eq!(d.editor.buffer.text.to_string(), "one");
    d.switch(b);
    assert_eq!(d.editor.buffer.text.to_string(), "😀two");
}
#[test]
fn dirty_close_and_stale_reload_are_safe() {
    let mut d = docs();
    let a = add(&mut d, Path::new("a.rs"), "old");
    d.editor.buffer.insert("edit");
    assert!(d.close_document(a, false).is_err());
    assert!(!d.reload(a, 0, Buffer::default()).unwrap());
    assert!(d.get(a).is_some());
    let rev = d.editor.buffer.revision;
    assert!(
        d.reload(
            a,
            rev,
            Buffer::new(Rope::from_str("new"), Some("a.rs".into()))
        )
        .unwrap()
    );
    assert_ne!(d.id(), a);
    assert!(d.get(a).is_none());
    assert_eq!(d.editor.buffer.text.to_string(), "new");
}
#[test]
fn opening_existing_document_preserves_unsaved_state() {
    let mut d = docs();
    let a = add(&mut d, Path::new("a.rs"), "one");
    d.editor.buffer.insert("change");
    add(&mut d, Path::new("b.rs"), "two");
    assert_eq!(add(&mut d, Path::new("a.rs"), "disk"), a);
    assert_eq!(d.order.len(), 2);
    assert_eq!(d.editor.buffer.text.to_string(), "changeone");
}
#[test]
fn async_save_updates_inactive_document_only() {
    let mut d = docs();
    let a = add(&mut d, Path::new("a.rs"), "one");
    d.editor.buffer.insert("α");
    let state = d.editor.buffer.state();
    let b = add(&mut d, Path::new("b.rs"), "two");
    io::apply(
        d.get_mut(a).unwrap(),
        Done::Save {
            disk_state: filavim::files::DiskState::Missing,
            document: a,
            path: "a.rs".into(),
            state,
            quit: false,
        },
    );
    assert!(!d.get(a).unwrap().buffer.dirty());
    assert_eq!(d.id(), b);
    assert_eq!(d.editor.buffer.text.to_string(), "two");
}
#[test]
fn workspace_edits_reject_all_when_one_document_changed() {
    let mut d = docs();
    let a = add(&mut d, Path::new("a.rs"), "one");
    let b = add(&mut d, Path::new("b.rs"), "two");
    let plan = vec![
        lsp::PlannedDocument {
            path: "a.rs".into(),
            disk_state: None,
            before: Rope::from_str("one"),
            revision: Some(0),
            edits: vec![(0..1, "O".into())],
        },
        lsp::PlannedDocument {
            path: "b.rs".into(),
            disk_state: None,
            before: Rope::from_str("two"),
            revision: Some(0),
            edits: vec![(0..1, "T".into())],
        },
    ];
    d.editor.buffer.insert("x");
    assert!(filavim::gui::services::apply_plan(&mut d, plan).is_err());
    assert_eq!(d.get(a).unwrap().buffer.text.to_string(), "one");
    assert!(d.get(b).unwrap().buffer.dirty());
}
#[test]
fn syntax_hidden_replacement_and_idle_do_not_accept_stale_results() {
    let mut d = docs();
    let id = add(&mut d, Path::new("a.rs"), "fn main() { let s = \"α😀\"; }");
    let mut s = Services::new(PathBuf::from("."));
    s.update(&d);
    d.editor.buffer.insert("// moved\n");
    s.update(&d);
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        s.poll(&mut d);
        if s.syntax_spans
            .get(&id)
            .is_some_and(|(rev, _)| *rev == d.editor.buffer.revision)
        {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(s.syntax_spans[&id].0, d.editor.buffer.revision);
    assert!(!s.syntax_spans[&id].1.is_empty());
    let revision = d.editor.buffer.revision;
    d.reload(
        id,
        revision,
        Buffer::new(Rope::from_str("plain"), Some("b.txt".into())),
    )
    .unwrap();
    s.update(&d);
    assert!(!s.syntax_spans.contains_key(&id));
    for _ in 0..10 {
        s.poll(&mut d);
    }
}
fn until(mut test: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if test() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("service timed out");
}
#[test]
fn native_lsp_mock_diagnostics_completion_and_stale_response() {
    let temp = tempfile::tempdir().unwrap();
    let exe = support::mock_executable(temp.path());
    let log = temp.path().join("messages.log");
    let mut config = Config::default();
    config.lsp.servers = vec![lsp::ServerConfig {
        name: "mock".into(),
        command: exe.to_string_lossy().into(),
        args: vec![
            log.to_string_lossy().into(),
            "--flood".into(),
            "--incremental".into(),
            "--slow-shutdown".into(),
            "--slow-resolve".into(),
        ],
        languages: vec!["rust".into()],
        ..Default::default()
    }];
    let mut second = config.lsp.servers[0].clone();
    second.name = "mock-python".into();
    second.languages = vec!["python".into()];
    second.args = vec![
        temp.path()
            .join("python.log")
            .to_string_lossy()
            .into_owned(),
    ];
    config.lsp.servers.push(second);
    let mut d = Documents::new(Model::new(config).unwrap());
    add(&mut d, &temp.path().join("b.py"), "v");
    let path = temp.path().join("a.rs");
    std::fs::write(&path, "v").unwrap();
    add(&mut d, &path, "v");
    let mut s = Services::new(temp.path().to_owned());
    s.update(&d);
    until(|| {
        s.poll(&mut d);
        s.diagnostics.len() == 2 && s.diagnostics.values().all(|(_, _, items)| items.len() == 1)
    });
    d.editor.buffer.cursor = 1;
    s.request(&d, Kind::Completion, None);
    until(|| {
        s.poll(&mut d);
        s.panel.is_some()
    });
    s.accept(&mut d);
    assert_eq!(
        d.editor.buffer.text, "v",
        "completion must await its resolved edits"
    );
    until(|| {
        s.poll(&mut d);
        d.editor.buffer.text == "variable"
    });
    assert_eq!(d.editor.buffer.text.to_string(), "variable");
    d.act(Action::Undo);
    assert_eq!(d.editor.buffer.text.to_string(), "v");
    s.update(&d);
    s.request(&d, Kind::Hover, None);
    until(|| {
        s.poll(&mut d);
        s.panel.as_ref().is_some_and(|p| p.kind == Kind::Hover)
    });
    assert!(
        s.panel.as_ref().unwrap().rows[0]
            .label
            .contains("Mock hover")
    );
    s.panel = None;
    s.request(&d, Kind::Formatting, None);
    until(|| {
        s.poll(&mut d);
        s.panel.as_ref().is_some_and(|p| p.kind == Kind::Formatting)
    });
    s.accept(&mut d);
    until(|| {
        s.poll(&mut d);
        d.editor.buffer.text == "F"
    });
    d.act(Action::Undo);
    assert_eq!(d.editor.buffer.text.to_string(), "v");
    s.saved(&d, d.id());
    until(|| {
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("textDocument/didSave")
    });
    s.request(&d, Kind::Completion, None);
    until(|| {
        s.poll(&mut d);
        s.panel.as_ref().is_some_and(|p| p.kind == Kind::Completion)
    });
    s.accept(&mut d);
    s.dismiss();
    for _ in 0..30 {
        s.poll(&mut d);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        d.editor.buffer.text, "v",
        "dismissed resolve must not insert a late completion"
    );
    assert!(s.panel.is_none());
    s.update(&d);
    s.request(&d, Kind::References, None);
    d.editor.buffer.insert("x");
    s.update(&d);
    for _ in 0..40 {
        s.poll(&mut d);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(s.panel.is_none());
    s.restart();
    assert!(
        !s.stopped(),
        "restarted server must remain owned during shutdown"
    );
    s.shutdown();
    until(|| s.stopped());
    assert!(std::fs::read_to_string(log).unwrap().contains("shutdown"));
}

#[test]
fn explorer_reuses_ignore_rules_and_discards_superseded_projects() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".ignore"), "ignored.txt\n").unwrap();
    std::fs::write(root.join("ignored.txt"), "hidden").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    let mut explorer = filavim::gui::explorer::Explorer::new(root.clone());
    explorer.open(temp.path().to_owned(), Config::default().navigation);
    explorer.open(root.clone(), Config::default().navigation);
    until(|| {
        explorer.poll();
        explorer.index.is_some() && !explorer.rows.is_empty()
    });
    assert_eq!(explorer.index.as_ref().unwrap().root, root);
    assert_eq!(explorer.index.as_ref().unwrap().files.len(), 1);
    let folder = explorer
        .rows
        .iter()
        .position(|r| r.entry.directory && r.entry.label == "src")
        .unwrap();
    assert!(explorer.activate(folder).is_none());
    until(|| {
        explorer.poll();
        explorer
            .rows
            .iter()
            .any(|r| r.entry.path.file_name().is_some_and(|n| n == "main.rs"))
    });
    let file = explorer
        .rows
        .iter()
        .position(|r| r.entry.path.file_name().is_some_and(|n| n == "main.rs"))
        .unwrap();
    assert_eq!(explorer.activate(file), Some(root.join("src/main.rs")));
}
#[test]
fn git_and_settings_only_touch_isolated_fixtures() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init"]).status.success());
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    let state = filavim::git::inspect(root, None, None).unwrap();
    assert_eq!(state.files.len(), 1);
    filavim::git::execute(filavim::git::Job::Stage {
        root: root.into(),
        path: root.join("a.rs"),
        undo: false,
    })
    .unwrap();
    assert!(
        git(&["diff", "--cached", "--name-only"])
            .stdout
            .starts_with(b"a.rs")
    );
    let path = root.join("config.toml");
    let config = filavim::setup::save_setting(&path, "font_size", "21").unwrap();
    assert_eq!(config.font_size, 21.);
    assert!(filavim::setup::save_setting(&path, "font_size", "900").is_err());
    assert_eq!(
        Config::parse(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .font_size,
        21.
    );
}

#[test]
fn completion_dynamic_readiness_and_ts_js_contexts_reach_the_popup() {
    let temp = tempfile::tempdir().unwrap();
    let exe = support::mock_executable(temp.path());
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    let log = temp.path().join("contexts.log");
    config.lsp.servers = vec![lsp::ServerConfig {
        name: "context-fixture".into(),
        command: exe.to_string_lossy().into(),
        args: vec![log.to_string_lossy().into(), "--dynamic-completion".into()],
        languages: vec!["typescript".into(), "javascript".into()],
        ..Default::default()
    }];
    let mut d = Documents::new(Model::new(config).unwrap());
    add(&mut d, &temp.path().join("first.ts"), "let value = v");
    d.editor.buffer.cursor = d.editor.buffer.text.len_chars();
    d.editor.set_mode(filavim::editor::Mode::Insert);
    let mut s = Services::new(temp.path().to_owned());
    s.manager = filavim::lsp_manager::Manager::at(temp.path().join("manager"), temp.path().into());
    s.update(&d);
    // No new keypress: registration after initialize must trigger the existing prefix.
    until(|| {
        s.poll(&mut d);
        s.panel
            .as_ref()
            .is_some_and(|p| p.rows.iter().any(|r| r.label.starts_with("variable ")))
    });
    for (index, (extension, source)) in [
        ("ts", "type X = { value: v§ }"),
        ("ts", "interface X { value: v§ }"),
        ("ts", "function f(arg: v§) {}"),
        ("ts", "function f(): v§ {}"),
        ("ts", "type X = Array<v§>"),
        ("ts", "class C { value: v§ }"),
        ("ts", "import { v§ } from 'fixture'"),
        ("js", "const config = { value: v§ }"),
        ("js", "function f() {\n const value =\n v§\n}"),
        ("js", "object.v§"),
        ("ts", "object?.§"),
    ]
    .into_iter()
    .enumerate()
    {
        s.dismiss();
        let cursor = source[..source.find('§').unwrap()].chars().count();
        add(
            &mut d,
            &temp.path().join(format!("case{index}.{extension}")),
            &source.replace('§', ""),
        );
        d.editor.buffer.cursor = cursor;
        d.editor.set_mode(filavim::editor::Mode::Insert);
        s.automatic_completion(&d);
        until(|| {
            s.poll(&mut d);
            s.panel
                .as_ref()
                .is_some_and(|p| p.rows.iter().any(|r| r.label.starts_with("variable ")))
        });
        assert_eq!(s.panel.as_ref().unwrap().document, d.id());
    }
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains("\"triggerKind\":2")
    );
    s.shutdown();
}
#[test]
fn completion_recovers_coalesced_typing_after_empty_error_and_out_of_order_replies() {
    let temp = tempfile::tempdir().unwrap();
    let exe = support::mock_executable(temp.path());
    for scenario in ["empty", "error", "filtered", "superseded"] {
        let log = temp.path().join(format!("{scenario}.log"));
        let mut config = Config {
            syntax: false,
            ..Default::default()
        };
        config.lsp.servers = vec![lsp::ServerConfig {
            name: format!("recover-{scenario}"),
            command: exe.to_string_lossy().into(),
            args: vec![
                log.to_string_lossy().into(),
                format!("--recover-{scenario}"),
            ],
            languages: vec!["rust".into()],
            ..Default::default()
        }];
        let mut d = Documents::new(Model::new(config).unwrap());
        add(&mut d, &temp.path().join("recover.rs"), "");
        d.editor.set_mode(filavim::editor::Mode::Insert);
        let mut s = Services::new(temp.path().to_owned());
        s.manager =
            filavim::lsp_manager::Manager::at(temp.path().join("manager"), temp.path().into());
        s.update(&d);
        until(|| {
            s.poll(&mut d);
            s.completion_ready(&d)
        });
        d.editor.type_character('a');
        s.automatic_completion(&d);
        until(|| {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("textDocument/completion")
        });
        for c in ['b', 'c'] {
            d.editor.type_character(c);
            s.automatic_completion(&d);
        }
        if scenario == "superseded" {
            s.request_completion(&d, true);
        }
        until(|| {
            s.poll(&mut d);
            s.panel
                .as_ref()
                .is_some_and(|p| p.rows.iter().any(|r| r.label.starts_with("abc_recovered")))
        });
        let session = s.panel.as_ref().unwrap().request;
        // Let the cancelled older reply arrive; it must not replace the new list.
        let deadline = Instant::now() + Duration::from_millis(130);
        while Instant::now() < deadline {
            s.poll(&mut d);
            thread::sleep(Duration::from_millis(2));
        }
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.request, session, "{scenario}");
        assert!(
            panel.rows.iter().all(|r| !r.label.starts_with("abc_old")),
            "{scenario}"
        );
        let requests = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|line| line.contains("textDocument/completion"))
            .count();
        assert_eq!(requests, 2, "no request buildup or retry loop: {scenario}");
        s.accept(&mut d);
        assert_eq!(d.editor.buffer.text, "abc_recovered", "{scenario}");
        s.shutdown();
    }
}
#[test]
fn completion_sessions_publish_progressively_rebase_and_cancel_across_documents() {
    let temp = tempfile::tempdir().unwrap();
    let exe = support::mock_executable(temp.path());
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.servers.clear();
    for (name, delay) in [("fast", "--session-fast"), ("slow", "--session-slow")] {
        config.lsp.servers.push(lsp::ServerConfig {
            name: name.into(),
            command: exe.to_string_lossy().into(),
            args: vec![
                temp.path()
                    .join(format!("{name}.log"))
                    .to_string_lossy()
                    .into(),
                delay.into(),
                "--incomplete".into(),
            ],
            languages: vec!["rust".into()],
            ..Default::default()
        });
    }
    let mut d = Documents::new(Model::new(config).unwrap());
    let first = add(&mut d, &temp.path().join("a.rs"), "");
    d.editor.set_mode(filavim::editor::Mode::Insert);
    let mut s = Services::new(temp.path().to_owned());
    s.update(&d);
    until(|| {
        s.poll(&mut d);
        ["fast", "slow"]
            .iter()
            .all(|id| s.runtime(id).iter().any(|r| r.state == "Running"))
    });
    d.editor.type_character('a');
    s.update(&d);
    s.automatic_completion(&d);
    d.editor.type_character('b');
    s.update(&d);
    s.automatic_completion(&d);
    until(|| {
        s.poll(&mut d);
        s.panel
            .as_ref()
            .is_some_and(|p| p.rows.iter().any(|r| r.label.starts_with("abc_fast")))
    });
    let session = s.panel.as_ref().unwrap().request;
    assert!(
        !s.panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.label.starts_with("abc_slow")),
        "fast provider must publish before the slow one"
    );
    for action in [Some('c'), None] {
        if let Some(c) = action {
            d.editor.type_character(c);
        } else {
            d.act(Action::Backspace);
        }
        s.update(&d);
        s.automatic_completion(&d);
        assert_eq!(
            s.panel.as_ref().unwrap().request,
            session,
            "typing/backspace must retain the popup identity"
        );
    }
    until(|| {
        s.poll(&mut d);
        s.panel
            .as_ref()
            .is_some_and(|p| p.rows.iter().any(|r| r.label.starts_with("abc_slow")))
    });
    assert_eq!(s.panel.as_ref().unwrap().request, session);
    let selected = s
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|r| r.label.starts_with("abc_slow"))
        .unwrap();
    s.panel.as_mut().unwrap().selected = selected;
    let identity = filavim::gui::completion::identity(&s.panel.as_ref().unwrap().rows[selected])
        .unwrap()
        .to_owned();
    d.editor.type_character('c');
    s.update(&d);
    s.automatic_completion(&d);
    let p = s.panel.as_ref().unwrap();
    assert_eq!(
        filavim::gui::completion::identity(&p.rows[p.selected]),
        Some(identity.as_str())
    );
    s.accept(&mut d);
    assert_eq!(
        d.editor.buffer.text, "abc_slow",
        "an old replacement range must include new prefix characters"
    );
    d.act(Action::Undo);
    assert_eq!(d.editor.buffer.text, "abc");
    s.dismiss();
    d.editor.buffer.cursor = 3;
    d.editor.type_character('.');
    s.update(&d);
    s.automatic_completion(&d);
    until(|| {
        std::fs::read_to_string(temp.path().join("fast.log"))
            .unwrap_or_default()
            .contains("\"triggerKind\":2")
    });
    let second = add(&mut d, &temp.path().join("b.rs"), "unrelated");
    s.update(&d);
    let deadline = Instant::now() + Duration::from_millis(230);
    while Instant::now() < deadline {
        s.poll(&mut d);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(s.panel.is_none());
    assert_eq!(d.editor.buffer.text, "unrelated");
    assert!(
        std::fs::read_to_string(temp.path().join("fast.log"))
            .unwrap()
            .contains("\"triggerKind\":3")
    );
    d.switch(first);
    s.update(&d);
    d.close_document(second, true).unwrap();
    s.update(&d);
    s.dismiss();
}

#[test]
fn native_resolved_actions_run_commands_only_after_validated_edits() {
    use filavim::gui::services::Choice;
    let temp = tempfile::tempdir().unwrap();
    let executable = support::mock_executable(temp.path());
    let log = temp.path().join("protocol.log");
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.servers = vec![lsp::ServerConfig {
        name: "milestone4".into(),
        command: executable.to_string_lossy().into_owned(),
        args: vec![log.to_string_lossy().into_owned()],
        languages: vec!["rust".into()],
        settings: serde_json::json!({"rust-analyzer":{"check":{"command":"clippy"}}}),
        ..Default::default()
    }];
    let mut docs = Documents::new(Model::new(config).unwrap());
    let path = temp.path().join("main.rs");
    std::fs::write(&path, "value").unwrap();
    add(&mut docs, &path, "value");
    let mut services = Services::new(temp.path().into());
    services.manager =
        filavim::lsp_manager::Manager::at(temp.path().join("tools"), temp.path().into());
    services.update(&docs);
    until(|| {
        services.poll(&mut docs);
        services.completion_ready(&docs)
    });
    services.request(&docs, Kind::Actions, None);
    until(|| {
        services.poll(&mut docs);
        services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::Actions)
    });
    services.panel.as_mut().unwrap().selected = 1;
    services.accept(&mut docs);
    until(|| {
        services.poll(&mut docs);
        services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::ActionResolve)
    });
    assert_eq!(docs.editor.buffer.text.to_string(), "value");
    services.accept(&mut docs);
    until(|| {
        services.poll(&mut docs);
        docs.editor.buffer.text == "Ralue"
    });
    until(|| {
        services.poll(&mut docs);
        services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::Execute)
    });
    assert!(docs.editor.buffer.dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "value");
    docs.editor.buffer.undo();
    assert_eq!(docs.editor.buffer.text.to_string(), "value");
    for kind in [
        Kind::Declaration,
        Kind::TypeDefinition,
        Kind::DocumentSymbols,
        Kind::WorkspaceSymbols,
    ] {
        services.request(&docs, kind, None);
        until(|| {
            services.poll(&mut docs);
            services.panel.as_ref().is_some_and(|p| p.kind == kind)
        });
        let choice = services.accept(&mut docs).unwrap();
        assert!(matches!(choice, Choice::Location { .. }));
    }
    services.request(&docs, Kind::Actions, None);
    until(|| {
        services.poll(&mut docs);
        services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::Actions)
    });
    docs.editor.buffer.insert("new");
    services.accept(&mut docs);
    assert_eq!(docs.editor.buffer.text.to_string(), "newvalue");
    services.shutdown();
    let wire = std::fs::read_to_string(&log).unwrap();
    assert!(wire.contains("codeAction/resolve"));
    assert!(wire.contains("workspace/executeCommand"));
    assert!(wire.contains("clippy"));
}

#[test]
fn workspace_edit_of_closed_file_keeps_baseline_for_safe_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("closed.rs");
    std::fs::write(&path, "old").unwrap();
    let edit = serde_json::json!({"changes":{lsp::uri(&path).unwrap():[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"new"}]}});
    let plan = lsp::plan_edit(&edit, &Default::default(), lsp::Encoding::Utf16).unwrap();
    let baseline = filavim::files::disk_state(&path).unwrap();
    let mut documents = docs();
    filavim::gui::services::apply_plan(&mut documents, plan).unwrap();
    let editor = documents.get(documents.find(&path).unwrap()).unwrap();
    assert_eq!(editor.buffer.disk_state.as_ref(), Some(&baseline));
    assert!(editor.buffer.dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    filavim::files::save_checked(
        &path,
        &editor.buffer.text,
        editor.buffer.disk_state.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    std::fs::write(&path, "external").unwrap();
    assert!(filavim::files::save_checked(&path, &editor.buffer.text, &baseline).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
}

#[test]
fn ordered_text_document_edits_use_previous_results_and_require_annotation_approval() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ordered.rs");
    std::fs::write(&path, "abc").unwrap();
    let uri = lsp::uri(&path).unwrap();
    let mut edit = serde_json::json!({"documentChanges":[
        {"textDocument":{"uri":uri,"version":null},"edits":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"newText":"日本"}]},
        {"textDocument":{"uri":uri,"version":null},"edits":[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"newText":"!"}]}
    ]});
    let plan = lsp::plan_edit(&edit, &Default::default(), lsp::Encoding::Utf16).unwrap();
    let mut documents = docs();
    filavim::gui::services::apply_plan(&mut documents, plan).unwrap();
    let id = documents.find(&path).unwrap();
    assert_eq!(documents.get(id).unwrap().buffer.text.to_string(), "a日本!");
    documents.get_mut(id).unwrap().buffer.undo();
    assert_eq!(documents.get(id).unwrap().buffer.text.to_string(), "abc");
    edit["documentChanges"][0]["edits"][0]["annotationId"] = serde_json::json!("approval");
    edit["changeAnnotations"] =
        serde_json::json!({"approval":{"label":"Sensitive change","needsConfirmation":true}});
    assert!(lsp::plan_edit(&edit, &Default::default(), lsp::Encoding::Utf16).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
}
