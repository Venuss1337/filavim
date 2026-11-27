use filavim::files::{self, DiskState};
use ropey::Rope;
use std::{fs, fs::FileTimes};

#[test]
fn timestamp_preserving_external_change_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.rs");
    fs::write(&path, "original").unwrap();
    let (_, baseline) = files::open_tracked(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "external").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(files::save_checked(&path, &Rope::from_str("editor"), &baseline).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
}
#[test]
fn deletion_and_replacement_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "original").unwrap();
    let (_, baseline) = files::open_tracked(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(files::save_checked(&path, &Rope::new(), &baseline).is_err());
    fs::write(&path, "replacement").unwrap();
    assert!(files::save_checked(&path, &Rope::new(), &baseline).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "replacement");
}
#[test]
fn successive_saves_require_the_latest_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.rs");
    let first = files::save_checked(&path, &Rope::from_str("first"), &DiskState::Missing).unwrap();
    let second = files::save_checked(&path, &Rope::from_str("second"), &first).unwrap();
    assert!(files::save_checked(&path, &Rope::from_str("stale"), &first).is_err());
    assert_eq!(files::disk_state(&path).unwrap(), second);
}
#[test]
fn new_paths_resolve_parent_without_creating_a_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let path = dir.path().join("nested/../new.rs");
    let resolved = files::resolve_path(&path).unwrap();
    assert_eq!(
        resolved,
        fs::canonicalize(dir.path()).unwrap().join("new.rs")
    );
    let (text, baseline) = files::open_tracked(&resolved).unwrap();
    assert_eq!(text.len_chars(), 0);
    assert_eq!(baseline, DiskState::Missing);
    assert!(!resolved.exists());
    assert!(files::resolve_path(&dir.path().join("absent/new.rs")).is_err());
    files::save_checked(&resolved, &Rope::from_str("new"), &baseline).unwrap();
}
#[test]
fn save_as_never_overwrites_an_untracked_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing");
    fs::write(&path, "keep").unwrap();
    assert!(files::save_checked(&path, &Rope::from_str("editor"), &DiskState::Missing).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "keep");
}
#[test]
fn readonly_failure_preserves_contents_and_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readonly");
    fs::write(&path, "keep").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    let original = permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let baseline = files::disk_state(&path).unwrap();
    let result = files::save_checked(&path, &Rope::from_str("editor"), &baseline);
    fs::set_permissions(&path, original).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
    assert_eq!(files::disk_state(&path).unwrap(), baseline);
}

#[test]
fn native_open_job_accepts_new_path_without_writing() {
    use filavim::gui::io::{Done, Job};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.rs");
    let done = Job::Open {
        path: path.clone(),
        document: 1,
        revision: 0,
    }
    .run()
    .unwrap();
    assert!(
        matches!(done, Done::Open { disk_state: DiskState::Missing, text, .. } if text.len_chars() == 0)
    );
    assert!(!path.exists());
}

#[test]
fn native_save_routes_conflict_without_marking_editor_clean() {
    use filavim::{
        buffer::Buffer,
        config::Config,
        editor::Editor,
        gui::io::{self, Done, Job},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.rs");
    fs::write(&path, "initial").unwrap();
    let (text, expected) = files::open_tracked(&path).unwrap();
    let mut editor = Editor::new(Config::default());
    editor.set_buffer(Buffer::new(text, Some(path.clone())));
    editor.buffer.insert("new");
    let state = editor.buffer.state();
    fs::write(&path, "external").unwrap();
    let done = Job::Save {
        path: path.clone(),
        text: editor.buffer.text.clone(),
        expected,
        protected: vec![],
        state,
        document: editor.document_id,
        quit: true,
    }
    .run()
    .unwrap();
    assert!(matches!(&done, Done::Conflict { .. }));
    assert!(!io::apply(&mut editor, done));
    assert!(editor.buffer.dirty());
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
}

#[test]
fn native_snapshot_save_retains_newer_edits_and_updates_disk_baseline() {
    use filavim::{
        config::Config,
        editor::Editor,
        gui::io::{self, Job},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.rs");
    let mut editor = Editor::new(Config::default());
    editor.buffer.insert("snapshot");
    let job = Job::Save {
        path: path.clone(),
        text: editor.buffer.text.clone(),
        expected: DiskState::Missing,
        protected: vec![],
        state: editor.buffer.state(),
        document: editor.document_id,
        quit: true,
    };
    editor.buffer.insert("newer");
    assert!(!io::apply(&mut editor, job.run().unwrap()));
    assert!(editor.buffer.dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "snapshot");
    assert_eq!(
        editor.buffer.disk_state,
        Some(files::disk_state(&path).unwrap())
    );
}

#[test]
fn native_save_as_rejects_canonical_alias_of_an_open_document() {
    use filavim::gui::io::Job;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let path = dir.path().join("existing.rs");
    fs::write(&path, "preserve").unwrap();
    let expected = files::disk_state(&path).unwrap();
    let result = Job::Save {
        path: dir.path().join("nested/../existing.rs"),
        text: Rope::from_str("overwrite"),
        expected,
        protected: vec![path.clone()],
        state: 1,
        document: 1,
        quit: false,
    }
    .run();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "preserve");
}

#[test]
fn identical_content_replacement_still_changes_the_tracked_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("same.rs");
    fs::write(&path, "same").unwrap();
    let original = files::disk_state(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, "same").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert_ne!(files::disk_state(&path).unwrap(), original);
    assert!(files::save_checked(&path, &Rope::from_str("editor"), &original).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "same");
}

#[path = "support/filesystem.rs"]
mod filesystem;

#[test]
fn replace_undo_save_then_external_edit_preserves_the_undone_version_and_dirty_state() {
    use filavim::{
        buffer::Buffer,
        config::Config,
        editor::Editor,
        gui::io::{self, Done, Job},
        search::{self, Options},
    };
    let workspace = filesystem::Workspace::new();
    let path = workspace.write("src/unicode.rs", "café=1\r\ncafé=2\r\n");
    let (text, baseline) = files::open_tracked(&path).unwrap();
    let mut editor = Editor::new(Config::default());
    editor.set_buffer(Buffer::new(text, Some(path.clone())));
    editor.buffer.disk_state = Some(baseline);
    let results = search::run(
        &editor.buffer.text,
        "café",
        Options::default(),
        0,
        false,
        Some(("日本", true)),
    )
    .unwrap();
    let id = editor.document_id;
    let revision = editor.buffer.revision;
    io::apply(
        &mut editor,
        Done::Search {
            results,
            document: id,
            revision,
        },
    );
    assert_eq!(editor.buffer.text.to_string(), "日本=1\r\n日本=2\r\n");
    editor.buffer.undo();
    assert_eq!(editor.buffer.text.to_string(), "café=1\r\ncafé=2\r\n");
    editor.buffer.redo();
    let save = editor.save(None, false).unwrap();
    io::apply(
        &mut editor,
        Job::from_request(save, id).unwrap().run().unwrap(),
    );
    assert!(!editor.buffer.dirty());
    editor.buffer.insert("newer");
    workspace.write("src/unicode.rs", "external");
    let failed = Job::from_request(editor.save(None, false).unwrap(), id)
        .unwrap()
        .run()
        .unwrap();
    assert!(matches!(failed, Done::Conflict { .. }));
    assert!(editor.buffer.dirty());
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
}
#[test]
fn independent_save_jobs_reject_a_stale_baseline_and_do_not_acknowledge_newer_edits() {
    use filavim::{
        config::Config,
        editor::Editor,
        gui::io::{self, Done, Job},
    };
    let workspace = filesystem::Workspace::new();
    let path = workspace.root().join("new.txt");
    let mut editor = Editor::new(Config::default());
    editor.buffer.insert("one");
    let id = editor.document_id;
    let first = Job::from_request(editor.save(Some(path.clone()), false).unwrap(), id).unwrap();
    let stale = Job::from_request(editor.save(Some(path.clone()), false).unwrap(), id).unwrap();
    editor.buffer.insert(" two");
    io::apply(&mut editor, first.run().unwrap());
    assert!(editor.buffer.dirty());
    assert!(matches!(stale.run().unwrap(), Done::Conflict { .. }));
    assert_eq!(fs::read_to_string(path).unwrap(), "one");
}

#[test]
fn frontend_preserves_parent_traversal_and_uses_platform_resolution() {
    use filavim::gui::io::{Done, Job};
    let workspace = filesystem::Workspace::new();
    let supplied = std::path::Path::new("missing/../new.rs");
    let path = filavim::paths::absolute(workspace.root(), supplied);
    assert_eq!(path, workspace.root().join(supplied));
    // Win32 accepts this normalized form; Unix requires the intermediate parent.
    // We preserve the supplied components and defer to the platform filesystem.
    let native_parent = fs::canonicalize(path.parent().unwrap());
    let result = Job::Open {
        path,
        document: 1,
        revision: 0,
    }
    .run();
    match native_parent {
        Ok(parent) => match result.unwrap() {
            Done::Open {
                path,
                text,
                disk_state,
                ..
            } => {
                assert_eq!(path, parent.join("new.rs"));
                assert_eq!(text.len_chars(), 0);
                assert_eq!(disk_state, DiskState::Missing);
            }
            _ => panic!("Expected an open result"),
        },
        Err(_) => assert!(result.is_err()),
    }
    assert!(!workspace.root().join("new.rs").exists());
}

#[cfg(unix)]
#[test]
fn parent_traversal_after_a_directory_symlink_follows_filesystem_semantics() {
    use std::os::unix::fs::symlink;
    let workspace = filesystem::Workspace::new();
    let target = workspace.write("actual/target.rs", "actual");
    std::fs::create_dir(workspace.root().join("actual/child")).unwrap();
    symlink(
        workspace.root().join("actual/child"),
        workspace.root().join("link"),
    )
    .unwrap();
    let path =
        filavim::paths::absolute(workspace.root(), std::path::Path::new("link/../target.rs"));
    assert_eq!(
        files::resolve_path(&path).unwrap(),
        std::fs::canonicalize(target).unwrap()
    );
}
