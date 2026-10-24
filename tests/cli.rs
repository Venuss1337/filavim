use filavim::{
    cli::{self, Cli, Launch},
    config::Config,
    files,
    storage::History,
};
use std::{
    ffi::OsString,
    fs,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn resolve(cwd: &std::path::Path, values: &[&str]) -> anyhow::Result<Launch> {
    let Cli::Open(paths) = Cli::parse(args(values))? else {
        panic!("expected launch arguments")
    };
    Launch::resolve(cwd, &paths)
}
fn config() -> Config {
    let mut c = Config::default();
    c.lsp.enabled = false;
    c.git.enabled = false;
    c
}

#[test]
fn parsing_is_native_minimal_and_has_explicit_option_termination() {
    assert_eq!(Cli::parse(args(&[])).unwrap(), Cli::Open(vec![]));
    for option in ["--help", "-h"] {
        assert_eq!(Cli::parse(args(&[option])).unwrap(), Cli::Help);
    }
    for option in ["--version", "-V"] {
        assert_eq!(Cli::parse(args(&[option])).unwrap(), Cli::Version);
    }
    assert_eq!(
        Cli::parse(args(&[".", "a b.rs", "日本.rs"])).unwrap(),
        Cli::Open(vec![".".into(), "a b.rs".into(), "日本.rs".into()])
    );
    assert_eq!(
        Cli::parse(args(&["--", "--help", "-file", "--"])).unwrap(),
        Cli::Open(vec!["--help".into(), "-file".into(), "--".into()])
    );
    for invalid in ["--unknown", "-x", "--version=1", ""] {
        assert!(Cli::parse(args(&[invalid])).is_err(), "{invalid}");
    }
}

#[test]
fn no_arguments_keep_the_launch_directory_and_dashboard_even_inside_a_project() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("Cargo.toml"), "").unwrap();
    let child = temp.path().join("child");
    fs::create_dir(&child).unwrap();
    let launch = resolve(&child, &[]).unwrap();
    assert_eq!(launch.root, fs::canonicalize(&child).unwrap());
    assert!(!launch.explicit_directory);
    assert!(launch.files.is_empty());
    let workspace = launch.workspace(config()).unwrap();
    assert!(workspace.editor().dashboard);
    assert_eq!(workspace.buffers.len(), 1);
}

#[test]
fn explicit_directory_overrides_discovery_without_rebasing_file_arguments() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("Cargo.toml"), "").unwrap();
    fs::create_dir(temp.path().join("project")).unwrap();
    fs::write(temp.path().join("first.rs"), "launch directory").unwrap();
    let launch = resolve(temp.path(), &["first.rs", "./project"]).unwrap();
    assert_eq!(
        launch.root,
        fs::canonicalize(temp.path().join("project")).unwrap()
    );
    assert_eq!(
        launch.files,
        vec![fs::canonicalize(temp.path().join("first.rs")).unwrap()]
    );
    assert!(launch.explicit_directory);
    let workspace = launch.workspace(config()).unwrap();
    assert_eq!(
        workspace.editor().buffer.text.to_string(),
        "launch directory"
    );
    let dot = resolve(temp.path(), &["."]).unwrap();
    assert_eq!(dot.root, fs::canonicalize(temp.path()).unwrap());
}

#[test]
fn files_discover_the_nearest_project_and_fall_back_to_launch_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let launch = resolve(temp.path(), &["my project/src/main.rs"]).unwrap();
    assert_eq!(launch.root, fs::canonicalize(&root).unwrap());
    fs::remove_file(root.join("Cargo.toml")).unwrap();
    assert_eq!(
        resolve(temp.path(), &["my project/src/main.rs"])
            .unwrap()
            .root,
        fs::canonicalize(temp.path()).unwrap()
    );
    fs::write(root.join("src/pyproject.toml"), "").unwrap();
    fs::write(root.join("Cargo.toml"), "").unwrap();
    assert_eq!(
        resolve(temp.path(), &["my project/src/main.rs"])
            .unwrap()
            .root,
        fs::canonicalize(root.join("src")).unwrap()
    );
}

#[test]
fn multiple_native_paths_open_distinct_buffers_with_first_focused_and_no_writes() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("日本 first.rs");
    let second = temp.path().join("żółw.rs");
    fs::write(&first, "first\r\n").unwrap();
    fs::write(&second, "second\n").unwrap();
    let launch = Launch::resolve(
        temp.path(),
        &[first.clone(), "żółw.rs".into(), "./日本 first.rs".into()],
    )
    .unwrap();
    assert_eq!(launch.files.len(), 2);
    let workspace = launch.workspace(config()).unwrap();
    assert_eq!(workspace.buffers.len(), 2);
    assert_eq!(workspace.active_buffer(), 1);
    assert_eq!(
        workspace.editor().buffer.path,
        Some(fs::canonicalize(&first).unwrap())
    );
    assert_eq!(workspace.editor().buffer.newline, "\r\n");
    assert!(!workspace.editor().dashboard);
    assert_eq!(workspace.buffers[&2].buffer.text.to_string(), "second\n");
    assert_eq!(fs::read_to_string(&first).unwrap(), "first\r\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "second\n");
}

#[test]
fn nonexistent_file_is_memory_only_until_an_explicit_save() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    let launch = resolve(temp.path(), &["src/../new file.rs"]).unwrap();
    let path = fs::canonicalize(temp.path()).unwrap().join("new file.rs");
    assert_eq!(launch.files, vec![path.clone()]);
    let mut workspace = launch.workspace(config()).unwrap();
    assert!(!path.exists());
    assert_eq!(workspace.editor().buffer.path, Some(path.clone()));
    assert!(!workspace.editor().dashboard);
    assert_eq!(workspace.editor().buffer.text.len_chars(), 0);
    workspace.editor_mut().buffer.insert("new content");
    assert!(!path.exists());
    assert!(workspace.editor().buffer.dirty());
    files::save(&path, &workspace.editor().buffer.text).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "new content");
}

#[test]
fn conflicts_invalid_parents_and_invalid_text_fail_before_app_initialization() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("a")).unwrap();
    fs::create_dir(temp.path().join("b")).unwrap();
    assert!(
        resolve(temp.path(), &["a", "b"])
            .unwrap_err()
            .to_string()
            .contains("Conflicting project directories")
    );
    assert!(resolve(temp.path(), &["a", "./a"]).is_ok());
    assert!(resolve(temp.path(), &["missing/child.rs"]).is_err());
    assert!(
        resolve(temp.path(), &["missing/"])
            .unwrap_err()
            .to_string()
            .contains("Directory does not exist")
    );
    fs::write(temp.path().join("invalid.rs"), [0xff, 0xfe]).unwrap();
    let launch = resolve(temp.path(), &["invalid.rs"]).unwrap();
    assert!(launch.workspace(config()).is_err());
    assert!(resolve(temp.path(), &["invalid.rs/child"]).is_err());
}

#[test]
fn directory_launch_initializes_existing_explorer_index_and_history_without_chdir() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("hello.rs"), "fn main() {}\n").unwrap();
    let before = std::env::current_dir().unwrap();
    let launch = resolve(temp.path(), &["project"]).unwrap();
    let root = launch.root.clone();
    let mut app = launch
        .into_app(
            config(),
            History::default(),
            temp.path().join("state/history.toml"),
        )
        .unwrap();
    assert!(app.explorer.visible && app.explorer.focused);
    assert!(app.workspace.editor().dashboard);
    assert_eq!(app.history.projects.first(), Some(&root));
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.indexing || app.explorer.rows.is_empty() || app.pending() {
        assert!(Instant::now() < deadline, "launch workers did not finish");
        app.poll();
        thread::sleep(Duration::from_millis(3));
    }
    assert_eq!(app.workspace.root, root);
    assert_eq!(app.index.root, root);
    assert_eq!(app.index.files.len(), 1);
    assert_eq!(app.explorer.rows[0].entry.path, root.join("hello.rs"));
    assert_eq!(fs::read_dir(&project).unwrap().count(), 1);
    assert_eq!(std::env::current_dir().unwrap(), before);
    let launch = resolve(temp.path(), &["project", "project/hello.rs"]).unwrap();
    let app = launch
        .into_app(
            config(),
            History::default(),
            temp.path().join("other/history.toml"),
        )
        .unwrap();
    assert!(app.explorer.visible && !app.explorer.focused);
    assert_eq!(
        app.history.files.first(),
        app.workspace.editor().buffer.path.as_ref()
    );
}

#[test]
fn executable_help_version_and_errors_never_enter_terminal_mode_or_load_config() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("invalid-config.toml");
    fs::write(&config_path, "invalid [ toml").unwrap();
    for (option, expected) in [
        ("--help", "Usage: filavim"),
        ("-h", "Usage: filavim"),
        ("--version", cli::version()),
        ("-V", cli::version()),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_filavim"))
            .arg(option)
            .current_dir(temp.path())
            .env("FILAVIM_CONFIG", &config_path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(expected));
        assert!(!stdout.contains('\x1b'));
        assert!(output.stderr.is_empty());
    }
    for (values, expected) in [
        (vec!["--invalid"], "Unknown option"),
        (vec!["missing/child.rs"], "parent"),
        (vec![""], "Empty path"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_filavim"))
            .args(values)
            .current_dir(temp.path())
            .env("FILAVIM_CONFIG", &config_path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains('\x1b'));
    }
    assert_eq!(fs::read_to_string(config_path).unwrap(), "invalid [ toml");
}

#[cfg(windows)]
#[test]
fn windows_absolute_paths_backslashes_and_root_relative_paths_resolve() {
    use std::path::PathBuf;
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "").unwrap();
    let expected = fs::canonicalize(temp.path().join("src/main.rs")).unwrap();
    assert_eq!(
        resolve(temp.path(), &[r".\src\main.rs"]).unwrap().files,
        vec![expected.clone()]
    );
    assert_eq!(
        Launch::resolve(temp.path(), std::slice::from_ref(&expected))
            .unwrap()
            .files,
        vec![expected.clone()]
    );
    let root_relative = temp
        .path()
        .components()
        .skip(1)
        .collect::<PathBuf>()
        .join("src/main.rs");
    assert!(root_relative.has_root() && !root_relative.is_absolute());
    assert_eq!(
        Launch::resolve(temp.path(), &[root_relative])
            .unwrap()
            .files,
        vec![expected]
    );
    assert!(
        resolve(temp.path(), &["C:main.rs"])
            .unwrap_err()
            .to_string()
            .contains("Drive-relative")
    );
}

#[cfg(unix)]
#[test]
fn native_non_utf8_names_and_symlinks_are_resolved_without_loss() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let temp = tempfile::tempdir().unwrap();
    let name = OsString::from_vec(b"name-\xff.rs".to_vec());
    fs::write(temp.path().join(&name), "text").unwrap();
    let Cli::Open(paths) = Cli::parse([name.clone()]).unwrap() else {
        panic!()
    };
    let launch = Launch::resolve(temp.path(), &paths).unwrap();
    assert_eq!(launch.files[0].file_name(), Some(name.as_os_str()));
    symlink(&launch.files[0], temp.path().join("alias")).unwrap();
    assert_eq!(
        resolve(temp.path(), &["alias"]).unwrap().files,
        launch.files
    );
    symlink("missing", temp.path().join("broken")).unwrap();
    assert!(
        resolve(temp.path(), &["broken"])
            .unwrap_err()
            .to_string()
            .contains("symbolic link")
    );
}

#[cfg(unix)]
#[test]
fn inaccessible_directories_are_errors_when_permissions_are_enforced() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0)).unwrap();
    let denied = fs::read_dir(&private).is_err();
    let result = resolve(temp.path(), &["private"]);
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    if denied {
        assert!(result.is_err());
    }
}
