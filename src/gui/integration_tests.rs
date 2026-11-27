//! Runs the actual EditorView on GPUI's TestWindow; never opens an OS window.
use super::*;
use crate::buffer::Buffer;
use ropey::Rope;
use std::prelude::v1::test;

fn fixture<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    path: Option<&str>,
) -> (Entity<EditorView>, &'a mut VisualTestContext) {
    let mut config = crate::config::Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.enabled = false;
    config.cursor.enabled = false;
    config.background.enabled = false;
    let mut model = Model::new(config).unwrap();
    model.set_buffer(Buffer::new(Rope::from_str(text), path.map(Into::into)));
    let (view, cx) = cx.add_window_view(|_, cx| EditorView::new(model, cx));
    cx.update(|window, app| {
        window.focus(&view.read(app).focus);
        window.activate_window();
    });
    cx.run_until_parked();
    (view, cx)
}
#[gpui::test(seeds(0, 23))]
fn gpui_modal_input_focus_and_unicode_undo(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "", None);
    cx.simulate_keystrokes("i");
    cx.simulate_input("café😀");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.mode, Mode::Insert);
        assert_eq!(v.model.editor.buffer.text.to_string(), "café😀");
    });
    cx.simulate_keystrokes("escape u");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "")
    });
    cx.simulate_keystrokes("ctrl-r");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "café😀")
    });
    cx.update(|w, app| assert!(view.read(app).focus.is_focused(w)));
}
#[gpui::test(seeds(0, 23))]
fn gpui_completion_persists_navigates_accepts_and_keeps_focus(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "type Config = { name: ", Some("fixture.ts"));
    view.update(cx, |v, _| {
        v.model.editor.buffer.cursor = v.model.editor.buffer.text.len_chars()
    });
    cx.simulate_keystrokes("i");
    cx.simulate_keystrokes("s");
    let session = view.read_with(cx, |v, _| v.services.panel.as_ref().unwrap().request);
    for key in ["t", "r", "backspace", "r"] {
        cx.simulate_keystrokes(key);
        view.read_with(cx, |v, _| {
            assert_eq!(v.services.panel.as_ref().unwrap().request, session);
            assert!(
                v.services
                    .panel
                    .as_ref()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.label.starts_with("string "))
            );
        });
    }
    cx.simulate_keystrokes("down up tab");
    view.read_with(cx, |v, _| {
        assert!(v.model.editor.buffer.text.to_string().ends_with("string"));
        assert!(v.services.panel.is_none());
    });
    cx.update(|w, app| assert!(view.read(app).focus.is_focused(w)));
    cx.simulate_keystrokes("backspace");
    view.read_with(cx, |v, _| {
        assert!(
            v.services.panel.is_some(),
            "next input can start a fresh session"
        )
    });
}
#[gpui::test]
fn gpui_async_save_preserves_newer_edits(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("saved.txt");
    let (view, cx) = fixture(cx, "seed", None);
    view.update(cx, |v, cx| {
        let request = v.model.editor.save(Some(path.clone()), false).unwrap();
        v.request(request, cx);
        // Edit before the deterministic executor is allowed to finish the save.
        v.model.editor.buffer.insert("new");
        v.changed(cx);
    });
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "seed");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "newseed");
        assert!(v.model.editor.buffer.dirty());
        assert!(!v.busy);
    });
}
#[gpui::test]
fn gpui_resize_selection_geometry_and_idle_layout_reuse(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, &"\tlet café = \"😀\";\n".repeat(200), None);
    cx.simulate_keystrokes("v d d");
    view.read_with(cx, |v, _| {
        assert!(v.model.selection().end > v.model.selection().start);
        assert!(v.caret_bounds().is_some());
    });
    cx.simulate_resize(size(px(640.), px(420.)));
    cx.run_until_parked();
    let before = view.read_with(cx, |v, _| {
        let cache = v.cache.as_ref().unwrap();
        assert!(cache.lines.len() < 50);
        v.shaped_rows
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(
            v.shaped_rows, before,
            "unchanged frame must reuse viewport shapes"
        )
    });
}

#[gpui::test]
fn gpui_async_toast_preserves_focus_commands_and_editor_cache(cx: &mut TestAppContext) {
    struct Host {
        editor: Entity<EditorView>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    AnyView::from(self.editor.clone())
                        .cached(StyleRefinement::default().size_full()),
                )
                .child(self.editor.read(cx).toasts.clone())
        }
    }
    let mut config = crate::config::Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.enabled = false;
    config.cursor.reduced_motion = true;
    config.background.enabled = false;
    let (host, cx) = cx.add_window_view(|w, cx| {
        let mut model = Model::new(config).unwrap();
        model.set_buffer(Buffer::default());
        let editor = cx.new(|cx| EditorView::new(model, cx));
        editor.update(cx, |v, cx| v.focus(w, cx));
        w.activate_window();
        Host { editor }
    });
    cx.run_until_parked();
    let editor = host.read_with(cx, |h, _| h.editor.clone());
    let shaped = editor.read_with(cx, |v, _| v.shaped_rows);
    assert!(
        shaped > 0,
        "exercise the editor viewport rather than the dashboard"
    );
    editor.update(cx, |v, cx| {
        v.model.editor.notify("Previous command result", false);
        let task = cx.background_executor().spawn(async {
            super::super::toast::Notification::new(
                super::super::toast::Category::Information,
                "Background event",
                "Finished safely",
            )
            .persistent()
        });
        cx.spawn(async move |view, cx| {
            let notification = task.await;
            view.update(cx, |v, cx| {
                v.toasts.update(cx, |t, cx| {
                    t.show(notification, cx);
                });
            })
            .unwrap();
        })
        .detach();
    });
    cx.run_until_parked();
    editor.read_with(cx, |v, app| {
        assert_eq!(v.toasts.read(app).manager.entries.len(), 1);
        assert_eq!(
            v.shaped_rows, shaped,
            "toast frames must not reshape editor text"
        );
        assert_eq!(v.model.editor.message, "Previous command result");
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("hello");
    cx.simulate_keystrokes("escape :");
    editor.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.mode, Mode::Command);
        assert_eq!(v.model.editor.buffer.text.to_string(), "hello");
    });
    cx.update(|w, app| assert!(editor.read(app).focus.is_focused(w)));
    cx.simulate_resize(size(px(460.), px(300.)));
    cx.run_until_parked();
    editor.update(cx, |v, cx| {
        v.model.editor.notify("Saved", false);
        v.toasts.update(cx, |t, cx| {
            let id = t.manager.entries[0].id;
            t.dismiss(id, cx);
        });
    });
    cx.run_until_parked();
    editor.read_with(cx, |v, app| {
        assert!(v.toasts.read(app).manager.entries.is_empty());
        assert_eq!(
            v.model.editor.message, "Saved",
            "command feedback stays in its own channel"
        );
    });
}

#[gpui::test]
fn gpui_external_conflict_keep_and_confirmed_overwrite(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("conflict.rs");
    std::fs::write(&path, "baseline").unwrap();
    let baseline = crate::files::disk_state(&path).unwrap();
    let (view, cx) = fixture(cx, "baseline", None);
    view.update(cx, |v, _| {
        v.model.editor.buffer.path = Some(path.clone());
        v.model.editor.buffer.disk_state = Some(baseline);
        v.model.editor.buffer.insert("editor ");
    });
    std::fs::write(&path, "external").unwrap();
    view.update(cx, |v, cx| {
        let request = v.model.editor.save(None, false).unwrap();
        v.request(request, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(matches!(
            v.confirmation.as_ref().map(|c| &c.decision),
            Some(Decision::Conflict {
                overwrite: false,
                ..
            })
        ));
        assert!(v.model.editor.buffer.dirty());
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    view.read_with(cx, |v, _| assert!(v.confirmation.is_none()));
    view.update(cx, |v, cx| {
        let request = v.model.editor.save(None, false).unwrap();
        v.request(request, cx);
    });
    cx.run_until_parked();
    // Keep is selected initially; move to Overwrite, then explicitly confirm.
    cx.simulate_keystrokes("right right enter");
    view.read_with(cx, |v, _| {
        assert!(matches!(
            v.confirmation.as_ref().map(|c| &c.decision),
            Some(Decision::Conflict {
                overwrite: true,
                ..
            })
        ))
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    cx.simulate_keystrokes("left enter");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "editor baseline");
    view.read_with(cx, |v, _| assert!(!v.model.editor.buffer.dirty()));
    cx.update(|w, app| assert!(view.read(app).focus.is_focused(w)));
}

#[gpui::test]
fn gpui_regex_replace_all_is_one_undo_and_stale_results_are_rejected(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "α=1\r\nβ=2\n", None);
    view.update(cx, |v, cx| {
        v.model.editor.last_search = r"(\p{L})=(\d)".into();
        v.model.editor.search_options.regex = true;
        v.document_search(false, None, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.model.editor.search_ranges.len(), 2));
    view.update(cx, |v, cx| {
        v.document_search(false, Some(("${2}:$1".into(), true)), cx)
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "1:α\r\n2:β\n")
    });
    cx.simulate_keystrokes("u");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "α=1\r\nβ=2\n")
    });
    cx.simulate_keystrokes("ctrl-r");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "1:α\r\n2:β\n")
    });
    view.update(cx, |v, cx| {
        v.document_search(false, Some(("stale".into(), true)), cx);
        v.model.editor.buffer.insert("newer");
        v.changed(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.model.editor.buffer.text.to_string().contains("newer"))
    });
    cx.update(|w, app| assert!(view.read(app).focus.is_focused(w)));
}

#[gpui::test]
fn gpui_comment_shortcut_and_python_indentation_preserve_native_input(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "if ready:", Some("fixture.py"));
    view.update(cx, |v, _| {
        v.model.editor.buffer.cursor = v.model.editor.buffer.text.len_chars()
    });
    cx.simulate_keystrokes("i enter");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "if ready:\n    ")
    });
    cx.simulate_input("run()");
    cx.simulate_keystrokes("escape ctrl-/");
    view.read_with(cx, |v, _| {
        assert_eq!(
            v.model.editor.buffer.text.to_string(),
            "if ready:\n    # run()"
        )
    });
    cx.simulate_keystrokes("shift-tab");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "if ready:\n# run()")
    });
    cx.update(|w, app| assert!(view.read(app).focus.is_focused(w)));
}

#[gpui::test]
fn gpui_recovery_opens_dirty_copy_and_changed_disk_enters_conflict_flow(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recovered.rs");
    std::fs::write(&path, "original").unwrap();
    let baseline = crate::files::disk_state(&path).unwrap();
    std::fs::write(&path, "external").unwrap();
    let (view, cx) = fixture(cx, "", None);
    view.update(cx, |v, cx| {
        v.confirmation = Some(Confirmation {
            document: v.model.id(),
            revision: 0,
            selected: 0,
            decision: Decision::Recovery(vec![crate::recovery::Entry {
                path: Some(path.clone()),
                baseline: Some(baseline),
                text: "recovered".into(),
                cursor: 2,
                timestamp: 0,
                source: temp.path().join("old-snapshot.json"),
            }]),
        });
        cx.notify();
    });
    cx.simulate_keystrokes("enter");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "recovered");
        assert!(v.model.editor.buffer.dirty());
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    cx.simulate_keystrokes("ctrl-s");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(matches!(
            v.confirmation.as_ref().map(|c| &c.decision),
            Some(Decision::Conflict { .. })
        ))
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    cx.simulate_keystrokes("escape i");
    cx.simulate_input("x");
    view.read_with(cx, |v, _| {
        assert!(v.model.editor.buffer.text.to_string().contains('x'))
    });
}

#[gpui::test]
fn gpui_forced_close_does_not_retire_a_pending_save_owner(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "pending", None);
    let id = view.read_with(cx, |v, _| v.model.id());
    view.update(cx, |v, _| v.busy = true);
    cx.update(|window, app| view.update(app, |v, cx| v.command("q!".into(), window, cx)));
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.id(), id);
        assert!(!v.quitting);
        assert_eq!(v.model.editor.buffer.text.to_string(), "pending");
        assert!(v.model.editor.error);
    });
    view.update(cx, |v, _| v.busy = false);
}

#[gpui::test(seeds(0, 23))]
fn gpui_active_search_debounces_edits_and_preserves_cursor_mode_and_query(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "match and match", None);
    view.update(cx, |v, cx| {
        v.model.editor.last_search = "match".into();
        v.document_search(false, None, cx);
    });
    cx.run_until_parked();
    view.update(cx, |v, _| v.model.editor.buffer.cursor = 0);
    cx.simulate_keystrokes("i");
    for _ in 0..20 {
        cx.simulate_input("x");
        cx.run_until_parked();
    }
    let revision = view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.last_search, "match");
        assert_eq!(v.model.editor.buffer.cursor, 20);
        assert_eq!(v.model.editor.mode, Mode::Insert);
        assert_ne!(
            v.model.editor.search_revision,
            v.model.editor.buffer.revision
        );
        assert_eq!(v.model.editor.search_ranges, vec![20..25, 30..35]);
        v.model.editor.buffer.revision
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(149));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_ne!(v.model.editor.search_revision, revision)
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(1));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.search_revision, revision);
        assert_eq!(v.model.editor.search_ranges, vec![20..25, 30..35]);
        assert_eq!(v.model.editor.buffer.cursor, 20);
        assert_eq!(v.model.editor.mode, Mode::Insert);
    });
}
#[gpui::test(seeds(0, 23))]
fn gpui_new_query_at_same_revision_supersedes_old_results_and_clear_cancels(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx, "old new new", None);
    view.update(cx, |v, cx| {
        v.model.editor.last_search = "old".into();
        v.document_search(false, None, cx);
        v.model.editor.last_search = "new".into();
        v.document_search(false, None, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.search_ranges, vec![4..7, 8..11])
    });
    view.update(cx, |v, cx| {
        v.document_search(false, None, cx);
        v.model.editor.last_search.clear();
        v.changed(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.model.editor.search_ranges.is_empty()));
}
#[gpui::test(seeds(0, 23))]
fn gpui_search_does_not_block_save_and_tab_switch_preserves_both_owners(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("saved.rs");
    let (view, cx) = fixture(cx, "match", None);
    let original = view.read_with(cx, |v, _| v.model.id());
    view.update(cx, |v, cx| {
        v.model.editor.last_search = "match".into();
        let save = v.model.editor.save(Some(path.clone()), false).unwrap();
        v.request(save, cx);
        assert!(v.busy);
        v.document_search(false, None, cx);
        v.model
            .add(Buffer::new(Rope::from_str("other"), None), true)
            .unwrap();
        v.changed(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text.to_string(), "other");
        assert!(v.model.editor.search_ranges.is_empty());
        assert!(!v.model.get(original).unwrap().buffer.dirty());
        assert_eq!(v.model.get(original).unwrap().last_search, "match");
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "match");
    view.update(cx, |v, cx| {
        v.model.switch(original);
        v.changed(cx);
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.search_ranges, vec![0..5])
    });
}
#[gpui::test]
fn gpui_cancel_application_close_preserves_multiple_dirty_documents(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "first", None);
    view.update(cx, |v, _| {
        v.model.editor.buffer.insert("dirty ");
        let mut buffer = Buffer::default();
        buffer.insert("second dirty");
        v.model.add(buffer, true).unwrap();
    });
    let ids = view.read_with(cx, |v, _| v.model.dirty_ids());
    cx.update(|window, app| {
        view.update(app, |v, cx| {
            assert!(!v.close(window, cx));
        })
    });
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.dirty_ids(), ids);
        assert!(!v.quit_when_clean);
        assert!(v.confirmation.is_none());
        assert!(
            v.model
                .get(ids[0])
                .unwrap()
                .buffer
                .text
                .to_string()
                .contains("dirty")
        );
        assert_eq!(
            v.model.get(ids[1]).unwrap().buffer.text.to_string(),
            "second dirty"
        );
    });
}

#[gpui::test]
fn gpui_invalid_search_does_not_rescan_on_idle_or_damage_command_input(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "text", None);
    view.update(cx, |v, cx| {
        v.model.editor.last_search = "[".into();
        v.model.editor.search_options.regex = true;
        v.document_search(false, None, cx);
        v.model.editor.set_mode(Mode::Command);
        v.model.editor.command = "unsubmitted command".into();
    });
    cx.run_until_parked();
    let generation = view.read_with(cx, |v, _| {
        assert!(v.model.editor.search_error);
        assert!(v.model.editor.search_ranges.is_empty());
        assert_eq!(v.model.editor.command, "unsubmitted command");
        v.search_generation
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    view.update(cx, |v, cx| v.changed(cx));
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(
            v.search_generation
                .load(std::sync::atomic::Ordering::Relaxed),
            generation
        );
        assert_eq!(v.model.editor.buffer.text.to_string(), "text");
        assert_eq!(v.model.editor.mode, Mode::Command);
    });
}

#[gpui::test]
fn gpui_language_manager_filters_and_restores_editor_focus(cx: &mut TestAppContext) {
    let isolated = tempfile::tempdir().unwrap();
    let (view, cx) = fixture(cx, "unsaved café", None);
    view.update(cx, |v, cx| {
        v.services.manager =
            crate::lsp_manager::Manager::at(isolated.path().join("tools"), isolated.path().into());
        v.model.editor.buffer.cursor = 4;
        v.open_manager(cx);
        assert_eq!(v.manager_ids().len(), crate::lsp_catalog::CATALOG.len());
    });
    cx.simulate_keystrokes("p y r i g h t");
    view.read_with(cx, |v, _| {
        let ids: Vec<_> = v.manager_ids().iter().map(|e| e.id).collect();
        assert!(ids.contains(&"pyright"));
        assert!(!ids.contains(&"clangd"));
        assert_eq!(v.model.editor.buffer.cursor, 4);
        assert_eq!(v.model.editor.buffer.text.to_string(), "unsaved café");
    });
    cx.simulate_keystrokes("escape");
    cx.update(|window, app| {
        assert!(!view.read(app).manager_ui.open);
        assert!(view.read(app).focus.is_focused(window));
    });
}
#[gpui::test]
fn gpui_snippet_tab_precedes_completion_and_preserves_final_stop(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "", None);
    view.update(cx, |v, cx| {
        let expansion = crate::snippet::expand("${1:name}(${2:arg})$0").unwrap();
        v.model.editor.set_mode(Mode::Insert);
        v.model.editor.buffer.insert(&expansion.text);
        v.model.editor.buffer.cursor = 0;
        v.model.editor.snippet = Some(crate::snippet::Session::new(
            expansion.groups,
            0,
            &v.model.editor.buffer,
        ));
        v.changed(cx);
    });
    cx.simulate_keystrokes("tab");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.selection(), Some(5..8))
    });
    cx.simulate_keystrokes("shift-tab");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.selection(), Some(0..4))
    });
    cx.simulate_keystrokes("tab tab");
    view.read_with(cx, |v, _| {
        assert!(v.model.editor.snippet.is_none());
        assert_eq!(v.model.editor.buffer.cursor, 9);
        assert_eq!(v.model.editor.buffer.text.to_string(), "name(arg)");
    });
}

fn workspace_stage(
    view: &Entity<EditorView>,
    cx: &mut VisualTestContext,
    root: &std::path::Path,
    value: serde_json::Value,
) {
    view.update(cx, |v, cx| {
        v.services.root = root.into();
        v.services
            .workspace_edit(&v.model, value, crate::lsp::Encoding::Utf16, None);
        v.services.wait_workspace_task();
        v.services.poll(&mut v.model);
        v.changed(cx);
    });
    cx.run_until_parked();
}
fn workspace_finish(view: &Entity<EditorView>, cx: &mut VisualTestContext) {
    view.update(cx, |v, cx| {
        v.services.wait_workspace_task();
        v.services.poll(&mut v.model);
        v.changed(cx);
    });
    cx.run_until_parked();
}

fn workspace_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("main.rs");
    std::fs::write(&path, "abc").unwrap();
    // Editor open jobs canonicalize the path before putting it in the document.
    // The headless fixture constructs the document directly, so do the same here.
    let path = crate::files::resolve_path(&path).unwrap();
    (temp, path)
}

#[gpui::test]
fn gpui_workspace_annotation_rejection_and_expansion_preserve_selection_and_focus(
    cx: &mut TestAppContext,
) {
    let (temp, path) = workspace_fixture();
    let (view, cx) = fixture(cx, "abc", path.to_str());
    view.update(cx, |v, _| {
        v.model.editor.buffer.cursor = 1;
        v.model.editor.set_mode(Mode::Visual);
        v.model.editor.anchor = Some(0);
    });
    let value = serde_json::json!({"documentChanges":[{"textDocument":{"uri":crate::lsp::uri(&path).unwrap(),"version":0},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"X","annotationId":"reason"}]}],"changeAnnotations":{"reason":{"label":"Rename field","description":"Public API change","needsConfirmation":true}}});
    workspace_stage(&view, cx, temp.path(), value);
    view.read_with(cx, |v, _| {
        assert!(v.confirmation.is_some(), "{}", v.model.editor.message);
        assert!(matches!(
            v.confirmation.as_ref().unwrap().decision,
            Decision::Workspace {
                expanded: false,
                ..
            }
        ));
        assert!(
            v.services.workspace_approval.as_ref().unwrap().reasons[0]
                .contains("Public API change")
        );
        assert_eq!(v.model.editor.buffer.text, "abc");
    });
    cx.simulate_keystrokes("d");
    view.read_with(cx, |v, _| {
        assert!(matches!(
            v.confirmation.as_ref().unwrap().decision,
            Decision::Workspace { expanded: true, .. }
        ))
    });
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |v, _| {
        assert!(v.confirmation.is_none());
        assert!(!v.services.workspace_active());
        assert_eq!(v.model.editor.buffer.cursor, 1);
        assert_eq!(v.model.editor.anchor, Some(0));
        assert_eq!(v.model.editor.buffer.text, "abc");
    });
    cx.update(|window, app| assert!(view.read(app).focus.is_focused(window)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
}
#[gpui::test]
fn gpui_workspace_approve_rename_blocks_input_and_save_until_reconciled(cx: &mut TestAppContext) {
    let (temp, path) = workspace_fixture();
    let renamed = path.with_file_name("renamed.rs");
    let (view, cx) = fixture(cx, "abc", path.to_str());
    view.update(cx, |v, _| {
        v.model.editor.buffer.disk_state = Some(crate::files::disk_state(&path).unwrap());
        v.model.editor.set_mode(Mode::Insert);
        v.model.editor.buffer.cursor = 2;
    });
    workspace_stage(
        &view,
        cx,
        temp.path(),
        serde_json::json!({"documentChanges":[{"kind":"rename","oldUri":crate::lsp::uri(&path).unwrap(),"newUri":crate::lsp::uri(&renamed).unwrap()}]}),
    );
    // Reject is the default choice; approval must be explicit.
    cx.simulate_keystrokes("left enter");
    view.read_with(cx, |v, _| assert!(v.services.workspace_executing));
    cx.simulate_input("unwanted");
    view.update(cx, |v, cx| {
        let request = v.model.editor.save(None, false).unwrap();
        v.request(request, cx);
        assert!(!v.busy);
    });
    workspace_finish(&view, cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text, "abc");
        assert_eq!(v.model.editor.buffer.cursor, 2);
        assert_eq!(
            v.model.find(&renamed),
            Some(v.model.id()),
            "{}",
            v.model.editor.message
        );
        assert!(!v.services.workspace_active());
    });
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "abc");
    cx.update(|window, app| assert!(view.read(app).focus.is_focused(window)));
}
#[gpui::test]
fn gpui_workspace_disk_change_during_annotation_approval_rejects_execution(
    cx: &mut TestAppContext,
) {
    let (temp, path) = workspace_fixture();
    let (view, cx) = fixture(cx, "abc", path.to_str());
    let value = serde_json::json!({"documentChanges":[{"textDocument":{"uri":crate::lsp::uri(&path).unwrap(),"version":0},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"X","annotationId":"a"}]}],"changeAnnotations":{"a":{"label":"Sensitive change","needsConfirmation":true}}});
    workspace_stage(&view, cx, temp.path(), value);
    std::fs::write(&path, "external").unwrap();
    cx.simulate_keystrokes("left enter");
    workspace_finish(&view, cx);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.editor.buffer.text, "abc");
        assert!(!v.services.workspace_active());
        assert!(
            v.model.editor.message.contains("Filesystem changed"),
            "{}",
            v.model.editor.message
        );
    });
}
