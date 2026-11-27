//! Manager/client integration using owned temp state and local mock processes only.
mod support;
use filavim::{
    buffer::Buffer,
    config::Config,
    gui::{
        documents::Documents,
        model::Model,
        services::{Kind, Services},
    },
    lsp_manager::Manager,
};
use ropey::Rope;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn until(mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if f() {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
    panic!("LSP integration deadline exceeded");
}
fn add(docs: &mut Documents, path: &Path) {
    std::fs::write(path, "value = 1\nvalue\n").unwrap();
    docs.add(
        Buffer::new(Rope::from_str("value = 1\nvalue\n"), Some(path.into())),
        true,
    )
    .unwrap();
}
#[test]
fn empty_editor_does_not_probe_tools_or_create_manager_state() {
    let temp = support::tempdir();
    let data = temp.path().join("tools");
    let docs = Documents::new(
        Model::new(Config {
            syntax: false,
            ..Default::default()
        })
        .unwrap(),
    );
    let mut services = Services::new(temp.path().into());
    services.manager = Manager::at(data.clone(), temp.path().into());
    services.update(&docs);
    assert!(!services.manager.busy);
    assert!(!services.manager.loaded);
    assert!(!data.exists());
}

#[test]
fn overloaded_client_stops_and_reports_failure_instead_of_losing_protocol_messages() {
    use filavim::lsp::{Client, Event, ServerConfig};
    let temp = support::tempdir();
    let executable = support::mock_executable(temp.path());
    let client = Client::start(
        ServerConfig {
            command: executable.to_string_lossy().into_owned(),
            args: vec![
                temp.path()
                    .join("overload.log")
                    .to_string_lossy()
                    .into_owned(),
            ],
            ..Default::default()
        },
        temp.path().into(),
        Duration::from_secs(5),
    );
    client.notify(
        "fixture/oversized",
        serde_json::Value::String("x".repeat(33 * 1024 * 1024)),
    );
    let mut failure = None;
    until(|| {
        for event in client.events.try_iter() {
            if let Event::Stopped(reason) = event {
                failure = Some(reason);
            }
        }
        failure.is_some()
    });
    assert!(failure.unwrap().contains("overloaded"));
    assert!(client.input_metrics.snapshot().peak_approximate_bytes < 32 * 1024 * 1024);
}

#[test]
fn rollback_activates_only_a_healthy_owned_generation() {
    use filavim::lsp_manager::{Installation, Launch, Operation, Store};
    let temp = support::tempdir();
    let data = temp.path().join("tools");
    let executable = support::mock_executable(temp.path());
    let log = temp.path().join("rollback.log");
    let generation = |version: &str| {
        let directory = data
            .join("packages/rust-analyzer")
            .join(format!("generation-{version}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(".filavim-owned"), "rust-analyzer").unwrap();
        let command = directory.join(executable.file_name().unwrap());
        std::fs::copy(&executable, &command).unwrap();
        Installation {
            directory,
            version: version.into(),
            launch: Launch {
                command,
                args: vec![log.to_string_lossy().into_owned()],
            },
            source: "test fixture".into(),
        }
    };
    let active = generation("2");
    let previous = generation("1");
    let mut state = Store::default();
    state
        .installed
        .insert("rust-analyzer".into(), active.clone());
    state.previous.insert("rust-analyzer".into(), previous);
    let receipt = data.join("state.json");
    std::fs::write(&receipt, serde_json::to_vec(&state).unwrap()).unwrap();
    let mut manager = Manager::at(data, temp.path().into());
    assert!(manager.submit(Operation::Rollback("rust-analyzer".into())));
    until(|| {
        manager.poll();
        !manager.busy
    });
    assert!(manager.error.is_none(), "{:?}", manager.error);
    assert_eq!(
        manager.snapshot.store.installed["rust-analyzer"].version,
        "1"
    );
    assert_eq!(
        manager.snapshot.store.previous["rust-analyzer"].version,
        "2"
    );

    std::fs::remove_file(active.launch.command).unwrap();
    let before = std::fs::read(&receipt).unwrap();
    assert!(manager.submit(Operation::Rollback("rust-analyzer".into())));
    until(|| {
        manager.poll();
        !manager.busy
    });
    assert!(manager.error.is_some());
    assert_eq!(std::fs::read(&receipt).unwrap(), before);
    assert_eq!(
        manager.snapshot.store.installed["rust-analyzer"].version,
        "1"
    );
}
#[test]
fn workspace_trust_gates_configured_processes_and_revocation_stops_them() {
    use filavim::lsp_manager::Operation;
    let temp = support::tempdir();
    let root = temp.path();
    let executable = support::mock_executable(root);
    let log = root.join("trust.log");
    let mut config = Config {
        syntax: false,
        ..Config::default()
    };
    config.lsp.servers = vec![filavim::lsp::ServerConfig {
        name: "trust-fixture".into(),
        command: executable.to_string_lossy().into_owned(),
        args: vec![log.to_string_lossy().into_owned()],
        languages: vec!["python".into()],
        ..Default::default()
    }];
    let mut docs = Documents::new(Model::new(config).unwrap());
    add(&mut docs, &root.join("first.py"));
    let mut services = Services::new(root.into());
    let data = root.join("tools");
    services.manager = Manager::at(data.clone(), root.into());
    services.update(&docs);
    until(|| {
        services.poll(&mut docs);
        services.manager.has_snapshot
    });
    assert!(!services.manager.snapshot.trusted);
    assert!(services.runtime("trust-fixture").is_empty());
    assert!(!log.exists());

    assert!(services.manager.submit(Operation::TrustWorkspace(true)));
    until(|| {
        services.poll(&mut docs);
        services
            .runtime("trust-fixture")
            .iter()
            .any(|s| s.state == "Running")
    });
    assert!(log.is_file());
    assert!(services.manager.submit(Operation::TrustWorkspace(false)));
    until(|| {
        services.poll(&mut docs);
        !services.manager.busy && services.runtime("trust-fixture").is_empty()
    });
    assert!(!services.manager.snapshot.trusted);
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["trusted_workspaces"], serde_json::json!({}));
    services.shutdown();
}

#[test]
fn a_replacement_directory_does_not_inherit_project_trust() {
    use filavim::lsp_manager::Operation;
    let temp = support::tempdir();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let mut manager = support::trusted_manager(temp.path().join("tools"), root.clone());
    std::fs::rename(&root, temp.path().join("old-project")).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert!(manager.submit(Operation::Refresh));
    until(|| {
        manager.poll();
        !manager.busy
    });
    assert!(!manager.snapshot.trusted);
}

#[test]
fn complementary_servers_share_roots_route_capabilities_and_stop_without_restart_loops() {
    let temp = support::tempdir();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("pyproject.toml"), "").unwrap();
    let executable = support::mock_executable(temp.path());
    let data = temp.path().join("tools");
    std::fs::create_dir(&data).unwrap();
    let primary_log = temp.path().join("primary.log");
    let ruff_log = temp.path().join("ruff.log");
    let state = serde_json::json!({"global":{"custom":{
        "basedpyright":{"command":executable,"args":[primary_log]},
        "ruff":{"command":executable,"args":[ruff_log]}
    }}});
    std::fs::write(data.join("state.json"), serde_json::to_vec(&state).unwrap()).unwrap();
    let config = Config {
        syntax: false,
        ..Config::default()
    };
    let mut docs = Documents::new(Model::new(config).unwrap());
    add(&mut docs, &root.join("one.py"));
    add(&mut docs, &root.join("two.py"));
    let mut services = Services::new(root.clone());
    services.manager = support::trusted_manager(data, root.clone());
    until(|| {
        services.poll(&mut docs);
        services.update(&docs);
        services
            .runtime("basedpyright")
            .iter()
            .all(|s| s.state == "Running")
            && services.runtime("basedpyright").len() == 1
            && services
                .runtime("ruff")
                .iter()
                .all(|s| s.state == "Running")
            && services.runtime("ruff").len() == 1
    });
    until(|| {
        services.poll(&mut docs);
        std::fs::read_to_string(&primary_log)
            .unwrap_or_default()
            .matches("textDocument/didOpen")
            .count()
            == 2
            && std::fs::read_to_string(&ruff_log)
                .unwrap_or_default()
                .matches("textDocument/didOpen")
                .count()
                == 2
    });
    services.request(&docs, Kind::Hover, None);
    until(|| {
        services.poll(&mut docs);
        services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::Hover)
    });
    assert!(
        std::fs::read_to_string(&primary_log)
            .unwrap()
            .contains("textDocument/hover")
    );
    assert!(
        !std::fs::read_to_string(&ruff_log)
            .unwrap()
            .contains("textDocument/hover")
    );
    services.dismiss();
    services.request(&docs, Kind::Formatting, None);
    until(|| {
        services.poll(&mut docs);
        std::fs::read_to_string(&ruff_log)
            .unwrap()
            .contains("textDocument/formatting")
    });
    assert!(
        !std::fs::read_to_string(&primary_log)
            .unwrap()
            .contains("textDocument/formatting")
    );
    let second = root.join("other-root");
    std::fs::create_dir(&second).unwrap();
    std::fs::write(second.join("pyproject.toml"), "").unwrap();
    add(&mut docs, &second.join("three.py"));
    until(|| {
        services.poll(&mut docs);
        services.update(&docs);
        services.runtime("basedpyright").len() == 2
            && services
                .runtime("basedpyright")
                .iter()
                .all(|s| s.state == "Running")
            && services.runtime("ruff").len() == 2
            && services
                .runtime("ruff")
                .iter()
                .all(|s| s.state == "Running")
    });
    services.control("ruff", false, false);
    services.update(&docs);
    assert!(services.runtime("ruff").is_empty());
    for _ in 0..10 {
        services.poll(&mut docs);
        services.update(&docs);
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        services.runtime("ruff").is_empty(),
        "Stop must not cause an automatic restart"
    );
    services.control("ruff", true, false);
    until(|| {
        services.poll(&mut docs);
        services.update(&docs);
        services.runtime("ruff").len() == 2
            && services
                .runtime("ruff")
                .iter()
                .all(|s| s.state == "Running")
    });
    services.shutdown();
    until(|| {
        services.poll(&mut docs);
        services.stopped()
    });
}

#[test]
fn crashed_server_retries_twice_then_requires_explicit_restart() {
    let temp = support::tempdir();
    let executable = support::mock_executable(temp.path());
    let log = temp.path().join("crash.log");
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.servers = vec![filavim::lsp::ServerConfig {
        name: "fixture-crash".into(),
        command: executable.to_string_lossy().into_owned(),
        args: vec![log.to_string_lossy().into_owned(), "--crash".into()],
        languages: vec!["rust".into()],
        ..Default::default()
    }];
    let path = temp.path().join("test.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut docs = Documents::new(Model::new(config).unwrap());
    docs.add(
        Buffer::new(Rope::from_str("fn main() {}\n"), Some(path)),
        true,
    )
    .unwrap();
    let mut services = Services::new(temp.path().into());
    services.manager = support::trusted_manager(temp.path().join("tools"), temp.path().into());
    until(|| {
        services.poll(&mut docs);
        services.update(&docs);
        services
            .runtime("fixture-crash")
            .first()
            .is_some_and(|s| s.state == "Failed" && s.failure.is_some())
    });
    for _ in 0..20 {
        services.poll(&mut docs);
        services.update(&docs);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        services
            .notifications
            .iter()
            .any(|n| n.category == filavim::gui::toast::Category::Error)
    );
    assert!(
        services.message.is_none(),
        "server failures must not overwrite command feedback"
    );
    assert_eq!(
        std::fs::read_to_string(&log)
            .unwrap()
            .matches("initialize")
            .count(),
        3
    );
    services.control("fixture-crash", true, false);
    until(|| {
        services.poll(&mut docs);
        services.update(&docs);
        std::fs::read_to_string(&log)
            .unwrap()
            .matches("initialize")
            .count()
            == 4
            && services
                .runtime("fixture-crash")
                .first()
                .is_some_and(|s| s.state == "Retry scheduled")
    });
    services.shutdown();
    until(|| {
        services.poll(&mut docs);
        services.stopped()
    });
}
