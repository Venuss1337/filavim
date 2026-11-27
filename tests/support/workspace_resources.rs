use super::*;
use filavim::lsp::workspace::{self, OpenDocument};
use serde_json::{Value, json};

#[cfg(windows)]
#[test]
fn case_only_file_and_directory_renames_preserve_identity_and_allow_following_edits() {
    for directory in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let from = temp
            .path()
            .join(if directory { "folder" } else { "lower.rs" });
        let to = temp
            .path()
            .join(if directory { "Folder" } else { "Lower.rs" });
        let file = if directory {
            std::fs::create_dir(&from).unwrap();
            from.join("a.rs")
        } else {
            from.clone()
        };
        let destination = if directory {
            to.join("a.rs")
        } else {
            to.clone()
        };
        std::fs::write(&file, "abc").unwrap();
        let mut d = docs();
        let id = tracked(&mut d, &file);
        d.editor.buffer.insert("dirty");
        let result = apply(
            temp.path(),
            &mut d,
            request(vec![
                json!({"kind":"rename","oldUri":uri(&from),"newUri":uri(&to)}),
                edit(&destination, 8, 8, "!"),
            ]),
        );
        assert!(result.failure.is_none(), "{:?}", result.response());
        assert_eq!(d.find(&destination), Some(id));
        assert_eq!(d.editor.buffer.text.to_string(), "dirtyabc!");
        assert_eq!(
            d.editor.buffer.path.as_ref().unwrap().file_name(),
            destination.file_name()
        );
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "abc");
        assert!(
            std::fs::read_dir(temp.path())
                .unwrap()
                .any(|e| e.unwrap().file_name() == to.file_name().unwrap())
        );
    }
}
#[test]
fn reconciliation_keeps_newer_text_but_still_maps_the_committed_resource_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("from.rs");
    let to = temp.path().join("to.rs");
    std::fs::write(&from, "abc").unwrap();
    let mut d = docs();
    let id = tracked(&mut d, &from);
    let plan = workspace::plan(
        &request(vec![
            edit(&from, 0, 1, "A"),
            json!({"kind":"rename","oldUri":uri(&from),"newUri":uri(&to)}),
        ]),
        temp.path(),
        snapshots(&d),
        lsp::Encoding::Utf16,
    )
    .unwrap();
    let result = workspace::execute(plan);
    assert!(result.failure.is_none());
    d.editor.buffer.insert("newer");
    let failure = filavim::gui::services::reconcile_workspace(&mut d, &result).unwrap_err();
    assert_eq!(failure.index, 0);
    assert_eq!(d.find(&to), Some(id));
    assert_eq!(d.editor.buffer.text.to_string(), "newerabc");
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "abc");
    d.editor.buffer.undo();
    assert_eq!(d.editor.buffer.text.to_string(), "abc");
}
#[test]
fn resource_notifications_match_filters_and_keep_protocol_order() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.rs");
    let b = temp.path().join("b.rs");
    let mut d = docs();
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![
            json!({"kind":"create","uri":uri(&a)}),
            json!({"kind":"rename","oldUri":uri(&a),"newUri":uri(&b)}),
            json!({"kind":"delete","uri":uri(&b)}),
        ]),
    );
    assert!(result.failure.is_none());
    let options = json!({"didCreate":{"filters":[{"scheme":"file","pattern":{"glob":"**/*.{rs,ts}","matches":"file"}}]},"didRename":{"filters":[{"pattern":{"glob":"**/*.RS","options":{"ignoreCase":true}}}]},"didDelete":{"filters":[{"pattern":{"glob":"**/*.rs"}}]}});
    let messages = workspace::notifications(&result, &options);
    assert_eq!(
        messages.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(),
        vec![
            "workspace/didCreateFiles",
            "workspace/didRenameFiles",
            "workspace/didDeleteFiles",
            "workspace/didChangeWatchedFiles"
        ]
    );
    assert_eq!(messages[1].1["files"][0]["oldUri"], uri(&a));
    assert_eq!(messages[1].1["files"][0]["newUri"], uri(&b));
    assert!(workspace::notifications(&result,&json!({"didCreate":{"filters":[{"scheme":"untitled","pattern":{"glob":"**/*"}}]},"didRename":{"filters":[{"pattern":{"glob":"**/*","matches":"folder"}}]},"didDelete":{"filters":[{"pattern":{"glob":"**/*.py"}}]}})).iter().all(|(method,_)|method=="workspace/didChangeWatchedFiles"));
}

#[cfg(windows)]
fn junction(path: &Path, destination: &Path) {
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            handle: *mut std::ffi::c_void,
            code: u32,
            input: *const std::ffi::c_void,
            size: u32,
            output: *mut std::ffi::c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }
    std::fs::create_dir(path).unwrap();
    let print: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let substitute: Vec<u16> = format!("\\??\\{}", filavim::paths::display_path(destination))
        .encode_utf16()
        .collect();
    let mut data = Vec::new();
    data.extend(0xA0000003u32.to_le_bytes());
    data.extend((8u16 + (substitute.len() as u16 + print.len() as u16 + 2) * 2).to_le_bytes());
    data.extend(0u16.to_le_bytes());
    for number in [
        0,
        substitute.len() as u16 * 2,
        (substitute.len() as u16 + 1) * 2,
        print.len() as u16 * 2,
    ] {
        data.extend(number.to_le_bytes());
    }
    for unit in substitute
        .iter()
        .chain(std::iter::once(&0))
        .chain(print.iter())
        .chain(std::iter::once(&0))
    {
        data.extend(unit.to_le_bytes());
    }
    let handle = std::fs::OpenOptions::new()
        .access_mode(0x40000000)
        .share_mode(0)
        .custom_flags(0x02000000 | 0x00200000)
        .open(path)
        .unwrap();
    let mut returned = 0;
    assert_ne!(
        unsafe {
            DeviceIoControl(
                handle.as_raw_handle(),
                0x000900A4,
                data.as_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        },
        0,
        "junction fixture: {}",
        std::io::Error::last_os_error()
    );
}
#[test]
fn parent_replacement_and_same_content_file_replacement_invalidate_approval() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    std::fs::create_dir(&parent).unwrap();
    let file = parent.join("new");
    let plan = workspace::plan(
        &request(vec![json!({"kind":"create","uri":uri(&file)})]),
        temp.path(),
        vec![],
        lsp::Encoding::Utf16,
    )
    .unwrap();
    std::fs::rename(&parent, temp.path().join("previous-parent")).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let result = workspace::execute(plan);
    assert!(result.failure.is_some());
    assert!(!file.exists());
    std::fs::write(&file, "same").unwrap();
    let plan = workspace::plan(
        &request(vec![json!({"kind":"delete","uri":uri(&file)})]),
        temp.path(),
        vec![],
        lsp::Encoding::Utf16,
    )
    .unwrap();
    filavim::files::save(&file, &Rope::from_str("same")).unwrap();
    let result = workspace::execute(plan);
    assert!(result.failure.is_some());
    assert!(file.exists());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "same");
}
#[cfg(windows)]
#[test]
fn junction_escape_before_planning_and_while_awaiting_approval_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = temp.path().join("junction");
    junction(&link, outside.path());
    let value = request(vec![json!({"kind":"create","uri":uri(&link.join("new"))})]);
    assert!(workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).is_err());
    assert!(!outside.path().join("new").exists());
    let parent = temp.path().join("parent");
    std::fs::create_dir(&parent).unwrap();
    let value = request(vec![
        json!({"kind":"create","uri":uri(&parent.join("new"))}),
    ]);
    let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
    std::fs::remove_dir(&parent).unwrap();
    junction(&parent, outside.path());
    let result = workspace::execute(plan);
    assert!(result.failure.is_some());
    assert!(result.applied.is_empty());
    assert!(!outside.path().join("new").exists());
}
#[test]
fn recovery_metadata_tracks_dirty_renames_and_untitled_deletion_without_new_typing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let original = root.join("a.rs");
    let renamed = root.join("b.rs");
    std::fs::write(&original, "saved").unwrap();
    let mut d = docs();
    let id = tracked(&mut d, &original);
    d.editor.buffer.insert("unsaved");
    let recovery_root = temp.path().join("recovery");
    let mut recovery = filavim::recovery::Recovery::new(recovery_root.clone());
    recovery.sync(&d);
    recovery
        .flush()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let snapshot = std::fs::read_dir(&recovery_root)
        .unwrap()
        .map(|e| e.unwrap().path().join(format!("{id}.json")))
        .find(|p| p.is_file())
        .unwrap();
    let state = d.editor.buffer.state();
    let result = apply(
        &root,
        &mut d,
        request(vec![
            json!({"kind":"rename","oldUri":uri(&original),"newUri":uri(&renamed)}),
        ]),
    );
    assert!(result.failure.is_none());
    assert_eq!(d.editor.buffer.state(), state);
    recovery.sync(&d);
    recovery
        .flush()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let entry = filavim::recovery::load(&snapshot).unwrap();
    assert_eq!(
        entry.path.as_ref().map(|p| lsp::path_key(p)),
        Some(lsp::path_key(&renamed))
    );
    assert_eq!(entry.text, "unsavedsaved");
    let result = apply(
        &root,
        &mut d,
        request(vec![json!({"kind":"delete","uri":uri(&renamed)})]),
    );
    assert!(result.failure.is_none());
    recovery.sync(&d);
    recovery
        .flush()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let entry = filavim::recovery::load(&snapshot).unwrap();
    assert!(entry.path.is_none());
    assert!(entry.baseline.is_none());
    assert_eq!(entry.text, "unsavedsaved");
    assert!(d.get(id).unwrap().buffer.dirty());
}
fn uri(path: &Path) -> String {
    lsp::uri(path).unwrap()
}
fn edit(path: &Path, start: u32, end: u32, text: &str) -> Value {
    json!({"textDocument":{"uri":uri(path),"version":null},"edits":[{"range":{"start":{"line":0,"character":start},"end":{"line":0,"character":end}},"newText":text}]})
}
fn request(changes: Vec<Value>) -> Value {
    json!({"documentChanges":changes})
}
fn snapshots(d: &Documents) -> Vec<OpenDocument> {
    d.order
        .iter()
        .filter_map(|id| {
            let e = d.get(*id)?;
            Some(OpenDocument {
                id: *id,
                path: e.buffer.path.clone()?,
                snapshot: lsp::Snapshot {
                    text: e.buffer.text.clone(),
                    revision: e.buffer.revision,
                },
                baseline: e.buffer.disk_state.clone(),
                dirty: e.buffer.dirty(),
            })
        })
        .collect()
}
fn tracked(d: &mut Documents, path: &Path) -> u64 {
    let (text, baseline) = filavim::files::open_tracked(path).unwrap();
    let mut buffer = Buffer::new(text, Some(path.into()));
    buffer.disk_state = Some(baseline);
    d.add(buffer, true).unwrap()
}
fn apply(root: &Path, d: &mut Documents, value: Value) -> workspace::Outcome {
    let plan = workspace::plan(&value, root, snapshots(d), lsp::Encoding::Utf16).unwrap();
    let result = workspace::execute(plan);
    filavim::gui::services::reconcile_workspace(d, &result).unwrap();
    result
}
#[test]
fn create_edit_rename_edit_is_ordered_and_text_is_undoable_unsaved_content() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("new.rs");
    let b = temp.path().join("renamed.rs");
    let mut d = docs();
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![
            json!({"kind":"create","uri":uri(&a)}),
            edit(&a, 0, 0, "é"),
            json!({"kind":"rename","oldUri":uri(&a),"newUri":uri(&b)}),
            edit(&b, 1, 1, "日本"),
        ]),
    );
    assert!(result.failure.is_none(), "{:?}", result.response());
    assert!(!a.exists());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "");
    let id = d.find(&b).unwrap();
    let e = d.get_mut(id).unwrap();
    assert_eq!(e.buffer.text.to_string(), "é日本");
    assert!(e.buffer.dirty());
    assert!(e.buffer.disk_state.is_some());
    e.buffer.undo();
    assert_eq!(e.buffer.text.to_string(), "");
    assert!(
        b.exists(),
        "Text undo must not pretend to undo filesystem operations"
    );
    e.buffer.redo();
    assert_eq!(e.buffer.text.to_string(), "é日本");
}
#[test]
fn create_and_rename_options_follow_overwrite_precedence_and_preserve_originals() {
    for (overwrite, ignore, success, changed) in [
        (false, false, false, false),
        (false, true, true, false),
        (true, false, true, true),
        (true, true, true, true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::write(&a, "source").unwrap();
        std::fs::write(&b, "destination").unwrap();
        let mut d = docs();
        let value = request(vec![
            json!({"kind":"create","uri":uri(&b),"options":{"overwrite":overwrite,"ignoreIfExists":ignore}}),
        ]);
        let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16);
        assert_eq!(plan.is_ok(), success);
        if let Ok(plan) = plan {
            let result = workspace::execute(plan);
            assert!(result.failure.is_none(), "{:?}", result.response());
            assert_eq!(
                std::fs::read_to_string(&b).unwrap(),
                if changed { "" } else { "destination" }
            );
            if changed {
                assert_eq!(
                    std::fs::read_to_string(result.backups[0].join("original")).unwrap(),
                    "destination"
                );
            }
        }
        std::fs::write(&b, "destination").unwrap();
        let value = request(vec![
            json!({"kind":"rename","oldUri":uri(&a),"newUri":uri(&b),"options":{"overwrite":overwrite,"ignoreIfExists":ignore}}),
        ]);
        let plan = workspace::plan(&value, temp.path(), snapshots(&d), lsp::Encoding::Utf16);
        assert_eq!(plan.is_ok(), success);
        if let Ok(plan) = plan {
            let result = workspace::execute(plan);
            assert!(result.failure.is_none(), "{:?}", result.response());
            filavim::gui::services::reconcile_workspace(&mut d, &result).unwrap();
            assert_eq!(a.exists(), !changed);
            assert_eq!(
                std::fs::read_to_string(&b).unwrap(),
                if changed { "source" } else { "destination" }
            );
            if changed {
                assert_eq!(
                    std::fs::read_to_string(result.backups[0].join("original")).unwrap(),
                    "destination"
                );
            }
        }
    }
}
#[test]
fn directory_rename_preserves_dirty_descendant_identity_cursor_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir(&old).unwrap();
    let file = old.join("a.rs");
    std::fs::write(&file, "abc").unwrap();
    let mut d = docs();
    let id = tracked(&mut d, &file);
    d.editor.buffer.cursor = 1;
    d.editor.anchor = Some(0);
    d.editor.buffer.insert("é");
    let cursor = d.editor.buffer.cursor;
    let anchor = d.editor.anchor;
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![
            json!({"kind":"rename","oldUri":uri(&old),"newUri":uri(&new)}),
        ]),
    );
    assert!(result.failure.is_none(), "{:?}", result.response());
    assert_eq!(d.find(&new.join("a.rs")), Some(id));
    assert!(d.find(&file).is_none());
    let e = d.get_mut(id).unwrap();
    assert_eq!(e.buffer.cursor, cursor);
    assert_eq!(e.anchor, anchor);
    assert_eq!(e.buffer.text.to_string(), "aébc");
    assert!(e.buffer.dirty());
    e.buffer.undo();
    assert_eq!(e.buffer.text.to_string(), "abc");
    assert_eq!(std::fs::read_to_string(new.join("a.rs")).unwrap(), "abc");
}
#[test]
fn deletion_keeps_open_dirty_text_recoverable_and_retains_recursive_backup() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("a.rs");
    std::fs::write(&path, "original").unwrap();
    let mut d = docs();
    let id = tracked(&mut d, &path);
    d.editor.buffer.insert("unsaved");
    let invalid = request(vec![json!({"kind":"delete","uri":uri(&directory)})]);
    assert!(workspace::plan(&invalid, temp.path(), snapshots(&d), lsp::Encoding::Utf16).is_err());
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![
            edit(&path, 0, 0, "more"),
            json!({"kind":"delete","uri":uri(&directory),"options":{"recursive":true}}),
        ]),
    );
    assert!(result.failure.is_none(), "{:?}", result.response());
    assert!(!directory.exists());
    let e = d.get(id).unwrap();
    assert!(e.buffer.path.is_none());
    assert!(e.buffer.dirty());
    assert_eq!(e.buffer.text.to_string(), "moreunsavedoriginal");
    assert_eq!(
        std::fs::read_to_string(result.backups[0].join("original/a.rs")).unwrap(),
        "original"
    );
    let missing = request(vec![json!({"kind":"delete","uri":uri(&path)})]);
    assert!(workspace::plan(&missing, temp.path(), vec![], lsp::Encoding::Utf16).is_err());
    let ignored = request(vec![
        json!({"kind":"delete","uri":uri(&path),"options":{"ignoreIfNotExists":true}}),
    ]);
    let result = apply(temp.path(), &mut d, ignored);
    assert!(result.failure.is_none(), "{:?}", result.response());
}
#[test]
fn invalid_late_operation_applies_nothing_and_approval_is_bound_to_disk_state() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::write(&a, "original").unwrap();
    let value = request(vec![
        json!({"kind":"create","uri":uri(&b)}),
        json!({"kind":"unknown","uri":uri(&a)}),
    ]);
    let failure = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16)
        .err()
        .unwrap();
    assert_eq!(failure.index, 1);
    assert!(!b.exists());
    let value = request(vec![
        json!({"kind":"rename","oldUri":uri(&a),"newUri":uri(&b)}),
    ]);
    let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
    assert!(!plan.confirmations.is_empty());
    std::fs::write(&a, "external").unwrap();
    let result = workspace::execute(plan);
    assert!(result.failure.is_some());
    assert!(result.applied.is_empty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "external");
    assert!(!b.exists());
}
#[test]
fn annotations_outside_workspace_and_invalid_uris_are_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("new.rs");
    let mut value = request(vec![
        json!({"kind":"create","uri":uri(&path),"annotationId":"reason"}),
    ]);
    assert!(workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).is_err());
    value["changeAnnotations"] = json!({"reason":{"label":"Create helper","description":"Used by refactor","needsConfirmation":true}});
    let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
    assert!(
        plan.confirmations
            .iter()
            .any(|s| s.contains("Create helper: Used by refactor"))
    );
    assert!(
        plan.confirmations
            .iter()
            .any(|s| s.contains("Outside workspace"))
    );
    assert!(!path.exists());
    for address in [
        "https://example.com/a",
        "file:///../escape",
        "file:///C:/folder/%2e%2e/escape",
        "not a URI",
        "file:///C:/a?query",
    ] {
        let value = request(vec![json!({"kind":"create","uri":address})]);
        assert!(
            workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).is_err(),
            "{address}"
        );
    }
}
#[test]
fn dirty_destination_and_stale_version_are_rejected_without_disk_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();
    let mut d = docs();
    tracked(&mut d, &b);
    d.editor.buffer.insert("dirty");
    for value in [
        request(vec![
            json!({"kind":"rename","oldUri":uri(&a),"newUri":uri(&b),"options":{"overwrite":true}}),
        ]),
        request(vec![
            json!({"kind":"create","uri":uri(&b),"options":{"overwrite":true}}),
        ]),
    ] {
        assert!(workspace::plan(&value, temp.path(), snapshots(&d), lsp::Encoding::Utf16).is_err());
    }
    let mut value = request(vec![edit(&b, 0, 0, "x")]);
    value["documentChanges"][0]["textDocument"]["version"] = json!(900);
    assert!(workspace::plan(&value, temp.path(), snapshots(&d), lsp::Encoding::Utf16).is_err());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
    assert_eq!(d.editor.buffer.text.to_string(), "dirtyb");
}
#[test]
fn readonly_mid_execution_failure_reports_prefix_and_preserves_every_version() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::write(&b, "keep").unwrap();
    let value = request(vec![
        json!({"kind":"create","uri":uri(&a)}),
        json!({"kind":"delete","uri":uri(&b)}),
    ]);
    let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
    let original_permissions = std::fs::metadata(&b).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&b, permissions).unwrap();
    let result = workspace::execute(plan);
    // Attribute changes can be rejected at initial verification or at the write
    // boundary, depending on the OS baseline representation; neither loses data.
    assert!(result.failure.is_some());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "keep");
    assert_eq!(result.response()["applied"], false);
    if a.exists() {
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "");
        assert_eq!(result.failure.as_ref().unwrap().index, 1);
        assert_eq!(result.applied.len(), 1);
    }
    std::fs::set_permissions(&b, original_permissions).unwrap();
}
fn mock_services(root: &Path, value: &Value) -> (Documents, Services, PathBuf, PathBuf) {
    let executable = support::mock_executable(root);
    let log = root.join("resource-wire.log");
    let payload = root.join("resource-request.json");
    std::fs::write(&payload, serde_json::to_vec(value).unwrap()).unwrap();
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.servers = vec![lsp::ServerConfig {
        name: "workspace-fixture".into(),
        command: executable.to_string_lossy().into_owned(),
        args: vec![
            log.to_string_lossy().into_owned(),
            "--workspace-edit".into(),
            payload.to_string_lossy().into_owned(),
            "--duplicate-workspace".into(),
            "--await-workspace".into(),
        ],
        languages: vec!["rust".into()],
        ..Default::default()
    }];
    let mut d = Documents::new(Model::new(config).unwrap());
    let path = root.join("main.rs");
    std::fs::write(&path, "value").unwrap();
    tracked(&mut d, &path);
    let mut s = Services::new(root.into());
    s.manager = filavim::lsp_manager::Manager::at(root.join("tools"), root.into());
    s.update(&d);
    until(|| {
        s.poll(&mut d);
        s.completion_ready(&d)
    });
    (d, s, log, path)
}
fn wire_response(log: &Path) -> Option<Value> {
    std::fs::read_to_string(log)
        .ok()?
        .lines()
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|v| v["id"] == 910 && v.get("result").is_some())
}
#[test]
fn server_apply_edit_rejection_and_stale_approval_reply_without_mutation() {
    for stale in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        let value = request(vec![json!({"kind":"delete","uri":uri(&path)})]);
        let (mut d, mut s, log, path) = mock_services(temp.path(), &value);
        s.request(
            &d,
            Kind::Execute,
            Some(json!({"command":"mock.fix","arguments":[]})),
        );
        until(|| {
            s.poll(&mut d);
            s.workspace_approval.is_some()
        });
        let token = s.workspace_approval.as_ref().unwrap().token;
        if stale {
            d.editor.buffer.insert("new");
        }
        s.workspace_answer(&d, token, stale);
        until(|| {
            s.poll(&mut d);
            wire_response(&log).is_some()
        });
        assert_eq!(wire_response(&log).unwrap()["result"]["applied"], false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "value");
        assert_eq!(
            d.editor.buffer.text.to_string(),
            if stale { "newvalue" } else { "value" }
        );
        s.shutdown();
    }
}
#[test]
fn client_action_and_duplicate_server_request_apply_once_and_remap_uris() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("main.rs");
    let renamed = temp.path().join("renamed.rs");
    let value = request(vec![
        json!({"kind":"rename","oldUri":uri(&path),"newUri":uri(&renamed)}),
        edit(&renamed, 5, 5, "!"),
    ]);
    let (mut d, mut s, log, path) = mock_services(temp.path(), &value);
    let id = d.id();
    s.request(&d, Kind::Actions, None);
    until(|| {
        s.poll(&mut d);
        s.panel.as_ref().is_some_and(|p| p.kind == Kind::Actions)
    });
    s.panel.as_mut().unwrap().selected = 1;
    s.accept(&mut d);
    until(|| {
        s.poll(&mut d);
        s.panel
            .as_ref()
            .is_some_and(|p| p.kind == Kind::ActionResolve)
    });
    s.accept(&mut d);
    until(|| {
        s.poll(&mut d);
        s.workspace_approval.is_some()
    });
    assert_eq!(d.editor.buffer.text.to_string(), "Ralue");
    let token = s.workspace_approval.as_ref().unwrap().token;
    s.workspace_answer(&d, token, true);
    until(|| {
        s.poll(&mut d);
        wire_response(&log).is_some()
    });
    s.update(&d);
    until(|| {
        std::fs::read_to_string(&log)
            .unwrap()
            .contains(&format!("\"uri\":\"{}\"", uri(&renamed)))
    });
    assert_eq!(wire_response(&log).unwrap()["result"]["applied"], true);
    assert_eq!(d.id(), id);
    assert_eq!(d.find(&renamed), Some(id));
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "value");
    assert_eq!(d.editor.buffer.text.to_string(), "Ralue!");
    let wire: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect();
    assert_eq!(
        wire.iter()
            .filter(|v| v["id"] == 910 && v.get("result").is_some())
            .count(),
        1,
        "Duplicate request must not be applied/responded twice while pending"
    );
    let closed = wire
        .iter()
        .position(|v| {
            v["method"] == "textDocument/didClose"
                && v["params"]["textDocument"]["uri"] == uri(&path)
        })
        .unwrap();
    let opened = wire
        .iter()
        .position(|v| {
            v["method"] == "textDocument/didOpen"
                && v["params"]["textDocument"]["uri"] == uri(&renamed)
        })
        .unwrap();
    assert!(closed < opened);
    assert!(wire.iter().any(|v| v["method"] == "textDocument/didOpen"
        && v["params"]["textDocument"]["text"] == "Ralue!"));
    s.shutdown();
}
#[test]
fn server_invalid_plan_and_pending_save_return_accurate_failure() {
    for busy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let created = temp.path().join("created.rs");
        let path = temp.path().join("main.rs");
        let value = request(vec![
            json!({"kind":"create","uri":uri(&created)}),
            json!({"kind":"nonsense","uri":uri(&path)}),
        ]);
        let (mut d, mut s, log, _) = mock_services(temp.path(), &value);
        s.file_busy = busy;
        s.request(&d, Kind::Execute, Some(json!({"command":"mock.fix"})));
        until(|| {
            s.poll(&mut d);
            wire_response(&log).is_some()
        });
        let response = wire_response(&log).unwrap();
        assert_eq!(response["result"]["applied"], false);
        if !busy {
            assert_eq!(response["result"]["failedChange"], 1);
        }
        assert!(!created.exists());
        assert_eq!(d.editor.buffer.text.to_string(), "value");
        s.shutdown();
    }
}

#[test]
#[ignore = "requires installed rust-analyzer and local Rust toolchain; no downloads or desktop"]
fn real_rust_analyzer_create_module_resource_action() {
    let executable =
        filavim::setup::executable("rust-analyzer").expect("Prerequisite: installed rust-analyzer");
    let version = std::process::Command::new(&executable)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        version.status.success(),
        "Prerequisite: working rust-analyzer"
    );
    println!(
        "REAL RESOURCE SERVER: {}",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname=\"filavim_resource_acceptance\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    let path = temp.path().join("src/main.rs");
    let source = "mod helper;\nfn main() {}\n";
    std::fs::write(&path, source).unwrap();
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.completion.enabled = false;
    config.lsp.servers = vec![lsp::ServerConfig {
        name: "real-resource".into(),
        command: executable.to_string_lossy().into_owned(),
        languages: vec!["rust".into()],
        initialization_options: json!({"cargo":{"offline":true},"checkOnSave":false,"procMacro":{"enable":false}}),
        ..Default::default()
    }];
    let mut d = Documents::new(Model::new(config).unwrap());
    let id = tracked(&mut d, &path);
    d.editor.buffer.cursor = 4;
    let mut s = Services::new(temp.path().into());
    s.manager = filavim::lsp_manager::Manager::at(temp.path().join("tools"), temp.path().into());
    s.update(&d);
    until(|| {
        s.poll(&mut d);
        s.completion_ready(&d)
    });
    let started = Instant::now();
    let mut requested = Instant::now() - Duration::from_secs(1);
    let mut accepted = false;
    let module = loop {
        s.poll(&mut d);
        s.update(&d);
        if let Some(approval) = &s.workspace_approval {
            let token = approval.token;
            s.workspace_answer(&d, token, true);
        }
        if let Some(panel) = &mut s.panel {
            if panel.kind == Kind::Actions && !accepted {
                if let Some(index) = panel
                    .rows
                    .iter()
                    .position(|r| r.label.to_ascii_lowercase().contains("create module"))
                {
                    panel.selected = index;
                    s.accept(&mut d);
                    accepted = true;
                }
            } else if accepted && panel.kind == Kind::ActionResolve {
                s.accept(&mut d);
            }
        }
        if !accepted && requested.elapsed() > Duration::from_millis(500) {
            s.request(&d, Kind::Actions, None);
            requested = Instant::now();
        }
        if accepted
            && !s.workspace_active()
            && let Some(module) = [
                temp.path().join("src/helper.rs"),
                temp.path().join("src/helper/mod.rs"),
            ]
            .into_iter()
            .find(|p| p.is_file())
        {
            break module;
        }
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "Create module action did not commit: {:?}; rows {:?}",
            s.message,
            s.panel
                .as_ref()
                .map(|p| p.rows.iter().map(|r| &r.label).collect::<Vec<_>>())
        );
        thread::sleep(Duration::from_millis(10));
    };
    println!(
        "REAL RESOURCE: Create module created {}",
        module.strip_prefix(temp.path()).unwrap().display()
    );
    let started = Instant::now();
    let mut requested = Instant::now() - Duration::from_secs(1);
    loop {
        s.poll(&mut d);
        let found=s.panel.as_ref().is_some_and(|p|p.kind==Kind::Definition && p.rows.iter().any(|r|matches!(&r.choice,filavim::gui::services::Choice::Location{path,..} if lsp::path_key(path)==lsp::path_key(&module))));
        if found {
            break;
        }
        if requested.elapsed() > Duration::from_millis(500) {
            s.request(&d, Kind::Definition, None);
            requested = Instant::now();
        }
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "Created module not recognized by real server: {:?}",
            s.message
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(d.id(), id);
    assert_eq!(d.editor.buffer.text.to_string(), source);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    println!(
        "REAL RESOURCE: definition resolves to created module; source document identity/content preserved"
    );
    s.shutdown();
    until(|| {
        s.poll(&mut d);
        s.stopped()
    });
}

#[test]
fn create_preserves_an_unsaved_document_already_owning_the_new_filename() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("new.rs");
    let mut d = docs();
    let id = tracked(&mut d, &path);
    d.editor.buffer.insert("unsaved");
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![json!({"kind":"create","uri":uri(&path)})]),
    );
    assert!(result.failure.is_none(), "{:?}", result.response());
    assert_eq!(d.find(&path), Some(id));
    assert_eq!(d.editor.buffer.text.to_string(), "unsaved");
    assert!(d.editor.buffer.dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}
#[test]
fn mixed_unicode_crlf_edits_project_selection_and_undo_without_reversing_rename() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("a.rs");
    let to = temp.path().join("b.rs");
    let text = "😀é\r\nnext\r\n";
    std::fs::write(&path, text).unwrap();
    let mut d = docs();
    let id = tracked(&mut d, &path);
    d.editor.set_mode(filavim::editor::Mode::Visual);
    d.editor.buffer.cursor = 2;
    d.editor.anchor = Some(0);
    let result = apply(
        temp.path(),
        &mut d,
        request(vec![
            edit(&path, 2, 3, "日本"),
            json!({"kind":"rename","oldUri":uri(&path),"newUri":uri(&to)}),
        ]),
    );
    assert!(result.failure.is_none());
    assert_eq!(d.find(&to), Some(id));
    assert_eq!(d.editor.buffer.text.to_string(), "😀日本\r\nnext\r\n");
    assert_eq!(d.editor.buffer.cursor, 3);
    assert_eq!(d.editor.anchor, Some(0));
    assert_eq!(std::fs::read_to_string(&to).unwrap(), text);
    d.editor.buffer.undo();
    assert_eq!(d.editor.buffer.text.to_string(), text);
    assert!(to.exists());
}
#[test]
fn changed_later_target_and_unexpected_destinations_reject_the_whole_plan() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::write(&b, "original").unwrap();
    let plan = workspace::plan(
        &request(vec![
            json!({"kind":"create","uri":uri(&a)}),
            json!({"kind":"delete","uri":uri(&b)}),
        ]),
        temp.path(),
        vec![],
        lsp::Encoding::Utf16,
    )
    .unwrap();
    std::fs::write(&b, "external").unwrap();
    let result = workspace::execute(plan);
    assert_eq!(result.failure.as_ref().unwrap().index, 1);
    assert!(result.applied.is_empty());
    assert!(!a.exists());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "external");
    for rename in [false, true] {
        let destination = temp.path().join(if rename { "renamed" } else { "created" });
        let value = if rename {
            json!({"kind":"rename","oldUri":uri(&b),"newUri":uri(&destination)})
        } else {
            json!({"kind":"create","uri":uri(&destination)})
        };
        let plan = workspace::plan(
            &request(vec![value]),
            temp.path(),
            vec![],
            lsp::Encoding::Utf16,
        )
        .unwrap();
        std::fs::write(&destination, "unexpected").unwrap();
        let result = workspace::execute(plan);
        assert!(result.failure.is_some());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "unexpected");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "external");
    }
}
#[test]
fn multiple_annotations_bind_to_changes_and_rejected_versions_never_mutate_disk() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.rs");
    let b = temp.path().join("b.rs");
    let value = json!({"documentChanges":[{"kind":"create","uri":uri(&a),"annotationId":"create"},edit(&a,0,0,"new"),{"kind":"rename","oldUri":uri(&a),"newUri":uri(&b),"annotationId":"rename"}],"changeAnnotations":{"create":{"label":"Create helper","needsConfirmation":true},"rename":{"label":"Move helper","description":"New module name","needsConfirmation":true}}});
    let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
    assert!(
        plan.confirmations
            .iter()
            .any(|s| s.contains("Change 1: Create helper"))
    );
    assert!(
        plan.confirmations
            .iter()
            .any(|s| s.contains("Change 3: Move helper: New module name"))
    );
    assert!(plan.steps[2].summary.contains("New module name"));
    assert!(!a.exists());
    assert!(!b.exists());
    let result = workspace::execute(plan);
    let mut d = docs();
    filavim::gui::services::reconcile_workspace(&mut d, &result).unwrap();
    assert!(result.failure.is_none());
    assert_eq!(
        d.get(d.find(&b).unwrap()).unwrap().buffer.text.to_string(),
        "new"
    );
}
#[test]
fn server_can_reuse_a_completed_request_id_and_partial_failures_report_the_index() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main.rs");
    let value = request(vec![json!({"kind":"delete","uri":uri(&main)})]);
    let (mut d, mut s, log, _) = mock_services(temp.path(), &value);
    s.request(&d, Kind::Execute, Some(json!({"command":"mock.fix"})));
    until(|| {
        s.poll(&mut d);
        s.workspace_approval.is_some()
    });
    let token = s.workspace_approval.as_ref().unwrap().token;
    s.workspace_answer(&d, token, false);
    until(|| wire_response(&log).is_some());
    let created = temp.path().join("created");
    let kept = temp.path().join("kept");
    std::fs::write(&kept, "keep").unwrap();
    let value = request(vec![
        json!({"kind":"create","uri":uri(&created)}),
        json!({"kind":"delete","uri":uri(&kept)}),
    ]);
    std::fs::write(
        temp.path().join("resource-request.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    s.request(&d, Kind::Execute, Some(json!({"command":"mock.fix"})));
    until(|| {
        s.poll(&mut d);
        s.workspace_approval.is_some()
    });
    let original = std::fs::metadata(&kept).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&kept, readonly).unwrap();
    let token = s.workspace_approval.as_ref().unwrap().token;
    s.workspace_answer(&d, token, true);
    let responses = || {
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|v| v["id"] == 910 && v.get("result").is_some())
            .collect::<Vec<_>>()
    };
    until(|| {
        s.poll(&mut d);
        responses().len() == 2
    });
    let responses = responses();
    assert_eq!(responses[0]["result"]["applied"], false);
    assert_eq!(responses[1]["result"]["applied"], false);
    assert_eq!(responses[1]["result"]["failedChange"], 1);
    assert!(created.is_file());
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "keep");
    assert_eq!(d.editor.buffer.text.to_string(), "value");
    std::fs::set_permissions(&kept, original).unwrap();
    s.shutdown();
}
#[test]
#[ignore = "opt-in release ordered workspace planning/rename timings; temporary 100-file tree"]
fn workspace_resource_operation_timings() {
    let mut planning = Vec::new();
    let mut execution = Vec::new();
    for _ in 0..7 {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("source");
        let to = temp.path().join("destination");
        std::fs::create_dir(&from).unwrap();
        for index in 0..100 {
            std::fs::write(
                from.join(format!("file{index}.rs")),
                "// 日本\r\n".repeat(128),
            )
            .unwrap();
        }
        let value = request(vec![
            json!({"kind":"rename","oldUri":uri(&from),"newUri":uri(&to)}),
        ]);
        let start = Instant::now();
        let plan = workspace::plan(&value, temp.path(), vec![], lsp::Encoding::Utf16).unwrap();
        planning.push(start.elapsed().as_micros());
        let start = Instant::now();
        let result = workspace::execute(plan);
        execution.push(start.elapsed().as_micros());
        assert!(result.failure.is_none());
        assert_eq!(std::fs::read_dir(&to).unwrap().count(), 100);
    }
    planning.sort();
    execution.sort();
    println!(
        "Workspace resource workload: 100 UTF-8/CRLF files, {} bytes/file, 7 samples, optimized={}; plan p50={}us max={}us; execute p50={}us max={}us",
        "// 日本\r\n".len() * 128,
        !cfg!(debug_assertions),
        planning[3],
        planning[6],
        execution[3],
        execution[6]
    );
}

#[test]
fn deleting_the_last_file_can_execute_a_command_and_acknowledge_its_nested_edit() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main.rs");
    let generated = temp.path().join("generated.rs");
    let value = request(vec![json!({"kind":"create","uri":uri(&generated)})]);
    let (mut d, mut s, log, _) = mock_services(temp.path(), &value);
    s.request(&d, Kind::Actions, None);
    until(|| {
        s.poll(&mut d);
        s.panel.as_ref().is_some_and(|p| p.kind == Kind::Actions)
    });
    let server = s.panel.as_ref().unwrap().server.clone().unwrap();
    s.workspace_edit(
        &d,
        request(vec![json!({"kind":"delete","uri":uri(&main)})]),
        lsp::Encoding::Utf16,
        Some((server, json!({"command":"mock.fix","arguments":[]}))),
    );
    until(|| {
        s.poll(&mut d);
        s.workspace_approval.is_some()
    });
    let token = s.workspace_approval.as_ref().unwrap().token;
    s.workspace_answer(&d, token, true);
    until(|| {
        s.poll(&mut d);
        s.update(&d);
        generated.exists() && wire_response(&log).is_some()
    });
    assert!(d.editor.buffer.path.is_none());
    assert_eq!(d.editor.buffer.text.to_string(), "value");
    assert!(d.editor.buffer.dirty());
    assert!(!main.exists());
    assert_eq!(wire_response(&log).unwrap()["result"]["applied"], true);
    until(|| {
        s.poll(&mut d);
        s.update(&d);
        // Retiring the runtime queues shutdown; the fixture observes it asynchronously.
        s.runtime("workspace-fixture").is_empty()
            && std::fs::read_to_string(&log).is_ok_and(|wire| {
                wire.lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .any(|message| message["method"] == "shutdown")
            })
    });
    let wire: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let command = wire
        .iter()
        .find(|v| v["method"] == "workspace/executeCommand")
        .unwrap();
    assert!(
        !wire
            .iter()
            .any(|v| v["method"] == "$/cancelRequest" && v["params"]["id"] == command["id"])
    );
    let reply = wire
        .iter()
        .position(|v| v["id"] == 910 && v["result"]["applied"] == true)
        .unwrap();
    let shutdown = wire.iter().position(|v| v["method"] == "shutdown").unwrap();
    assert!(reply < shutdown);
    s.shutdown();
}
#[test]
fn catalog_rename_reuses_the_running_process_during_asynchronous_path_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    let from = root.join("old.rs");
    let to = root.join("new.rs");
    std::fs::write(&from, "abc").unwrap();
    let executable = support::mock_executable(root);
    let log = root.join("catalog-wire.log");
    let tools = root.join("tools");
    std::fs::create_dir(&tools).unwrap();
    std::fs::write(tools.join("state.json"),serde_json::to_vec(&json!({"global":{"custom":{"rust-analyzer":{"command":executable,"args":[log]}},"primary":{"rust":"rust-analyzer"}}})).unwrap()).unwrap();
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.completion.enabled = false;
    let mut d = Documents::new(Model::new(config).unwrap());
    let id = tracked(&mut d, &from);
    let mut s = Services::new(root.into());
    s.manager = filavim::lsp_manager::Manager::at(tools, root.into());
    until(|| {
        s.update(&d);
        s.poll(&mut d);
        s.completion_ready(&d)
    });
    s.workspace_edit(
        &d,
        request(vec![
            json!({"kind":"rename","oldUri":uri(&from),"newUri":uri(&to)}),
        ]),
        lsp::Encoding::Utf16,
        None,
    );
    until(|| {
        s.poll(&mut d);
        s.workspace_approval.is_some()
    });
    let token = s.workspace_approval.as_ref().unwrap().token;
    s.workspace_answer(&d, token, true);
    until(|| {
        s.poll(&mut d);
        s.update(&d);
        std::fs::read_to_string(&log)
            .unwrap()
            .contains(&format!("\"uri\":\"{}\"", uri(&to)))
    });
    assert_eq!(d.find(&to), Some(id));
    let wire: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(
        wire.iter().filter(|v| v["method"] == "initialize").count(),
        1
    );
    s.shutdown();
}
