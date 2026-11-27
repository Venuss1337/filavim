//! GPUI bootstrap: embedded font registration, native window creation and close routing.
//!
//! The root keeps a cached editor entity and a separate cursor entity as siblings,
//! so cursor-only frames need not rebuild editor content. Unsaved-close decisions
//! are delegated to `EditorView`; smoke entry points exercise this same window path.
use super::{FONT_BOLD_BYTES, FONT_BYTES, model, view};
use gpui::{prelude::*, *};
use std::{borrow::Cow, path::PathBuf};
/// Cache the editor between two independently animated siblings: the stage
/// beneath it (world, slab material, mode light) and the cursor above it. Their
/// frames never rebuild tabs, text, popup layout or input handlers.
pub(super) struct NativeWindow {
    stage: Entity<super::stage::Stage>,
    editor: Entity<view::EditorView>,
    cursor: Entity<super::cursor_view::CursorView>,
    toasts: Entity<super::toast_view::ToastView>,
}
impl NativeWindow {
    pub(super) fn with_editor<R>(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&mut view::EditorView, &mut Window, &mut Context<view::EditorView>) -> R,
    ) -> R {
        self.editor.update(cx, |v, cx| f(v, window, cx))
    }
}
impl Render for NativeWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .size_full()
                    // Cached: typing re-renders the editor, not the world beneath it.
                    .child(
                        AnyView::from(self.stage.clone())
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
            .child(
                AnyView::from(self.editor.clone()).cached(StyleRefinement::default().size_full()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .size_full()
                    .child(self.cursor.clone()),
            )
            .child(self.toasts.clone())
    }
}
pub fn run(path: Option<PathBuf>, config: crate::config::Config) -> anyhow::Result<()> {
    run_inner(path, config, None, None)
}
/// Bounded native startup/paint/idle smoke test; never edits or saves the fixture.
pub fn smoke(path: PathBuf, report: PathBuf) -> anyhow::Result<()> {
    std::fs::write(&report, "starting GPUI\n")?;
    run_inner(
        Some(path),
        {
            let mut config = crate::config::Config::default();
            config.lsp.enabled = false;
            match std::env::var("FILAVIM_CURSOR_SMOKE").as_deref() {
                Ok("off") => config.cursor.enabled = false,
                Ok("smooth") => {
                    config.cursor.smear = false;
                }
                Ok("all") => config.cursor.smear = true,
                _ => {}
            }
            config
        },
        Some(report.clone()),
        None,
    )?;
    let text = std::fs::read_to_string(report)?;
    anyhow::ensure!(
        text.contains("input_and_undo=Ok(Ok(()))")
            && text.contains("resize_and_cursor_geometry=Ok(Ok(()))")
            && text.contains("project_ready=Ok(Ok(()))")
            && text.contains("picker_ready=Ok(Ok(()))")
            && text.contains("floating_confirmation=Ok(Ok(()))")
            && text.contains("floating_information=Ok(Ok(()))")
            && text.contains("cursor_motion=Ok(())"),
        "native smoke checks failed: {text}"
    );
    Ok(())
}
pub fn smoke_lsp(path: PathBuf, report: PathBuf) -> anyhow::Result<()> {
    let mut config = crate::config::Config::default();
    config.lsp.servers.retain(|s| s.name == "rust");
    config.lsp.servers[0].initialization_options = serde_json::json!({"checkOnSave":false,"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false}});
    let tools = tempfile::tempdir()?;
    run_inner(
        Some(path),
        config,
        Some(report.clone()),
        Some(tools.path().into()),
    )?;
    let text = std::fs::read_to_string(report)?;
    anyhow::ensure!(text.contains("real_lsp_popup_smoke=PASS"), "{text}");
    Ok(())
}
fn run_inner(
    path: Option<PathBuf>,
    config: crate::config::Config,
    report: Option<PathBuf>,
    lsp_smoke_tools: Option<PathBuf>,
) -> anyhow::Result<()> {
    let model = model::Model::new(config)?;
    let failure = std::rc::Rc::new(std::cell::RefCell::new(None));
    let result = failure.clone();
    Application::new().run(move |cx: &mut App| {
        if let Some(p) = &report {
            let _ = std::fs::write(p, "application callback\n");
        }
        if let Err(e) = cx
            .text_system()
            .add_fonts(vec![Cow::Borrowed(FONT_BYTES), Cow::Borrowed(FONT_BOLD_BYTES)]) {
            *failure.borrow_mut()=Some(anyhow::anyhow!("Cannot load bundled Agave font: {e}"));
            cx.quit();
            return;
        }
        if let Some(p) = &report {
            let _ = std::fs::write(p, "font loaded\n");
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1100.), px(740.)), cx);
        let lsp_smoke = lsp_smoke_tools.is_some();
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(420.), px(240.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Filavim".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let editor = cx.new(|cx| view::EditorView::new(model, cx));
                let weak = editor.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    weak.update(cx, |v, cx| v.close(window, cx)).unwrap_or(true)
                });
                editor.update(cx, |view, cx| {
                    view.focus(window, cx);
                    if let Some(data) = lsp_smoke_tools {
                        view.set_lsp_smoke_storage(data);
                    }
                    if let Some(path) = path {
                        view.open_initial(path, cx);
                    }
                });
                let cursor=editor.read(cx).cursor.clone();
                let toasts=editor.read(cx).toasts.clone();
                let stage=editor.read(cx).stage.clone();
                cx.new(|_|NativeWindow{stage,editor,cursor,toasts})
            },
        );
        match result {
            Err(e) => {
                *failure.borrow_mut()=Some(anyhow::anyhow!("Cannot open Filavim window: {e:#}"));
                cx.quit();
            }
            Ok(window) => {
                if let Some(report) = report {
                    let executor = cx.background_executor().clone();
                    if lsp_smoke {
                        cx.spawn(async move|cx|{
                            let mut stage=0;let mut failure=None;
                            for tick in 0..600 {executor.timer(std::time::Duration::from_millis(100)).await;
                                match window.update(cx,|root,w,cx|root.with_editor(w,cx,|v,w,cx|v.lsp_smoke_step(stage,tick,w,cx))){Ok(Ok(next))=>stage=next,other=>{failure=Some(format!("{other:?}"));break;}}
                                if stage==9{break;}
                            }
                            let summary=window.update(cx,|root,w,cx|root.with_editor(w,cx,|v,_,cx|v.smoke_summary(cx))).unwrap_or_default();
                            let text=format!("real_lsp_popup_smoke={} stage={} failure={failure:?}\n{summary}",if stage==9&&failure.is_none(){"PASS"}else{"FAIL"},stage);
                            let _=executor.spawn(async move{std::fs::write(report,text)}).await;
                            let _=window.update(cx,|root,w,cx|root.with_editor(w,cx,|v,_,cx|v.smoke_shutdown(cx)));
                        }).detach();cx.activate(true);return;
                    }
                    cx.spawn(async move |cx| {
                        executor.timer(std::time::Duration::from_secs(3)).await;
                        let exercised = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_edit(window, cx)));
                        executor.timer(std::time::Duration::from_millis(250)).await;
                        let geometry = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_geometry(window, cx)));
                        let popup = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_popup(window,cx)));
                        executor.timer(std::time::Duration::from_millis(200)).await;
                        let popup_ready = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_popup_ready(window,cx)));
                        executor.timer(std::time::Duration::from_millis(200)).await;
                        let confirmation = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_confirmation_ready(window,cx)));
                        executor.timer(std::time::Duration::from_millis(200)).await;
                        let information = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, _, cx| view.smoke_information_ready(cx)));
                        let project = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, _, cx| view.smoke_project(cx)));
                        executor.timer(std::time::Duration::from_secs(2)).await;
                        let project_ready = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, window, cx| view.smoke_project_ready(window,cx)));
                        executor.timer(std::time::Duration::from_millis(250)).await;
                        let mut motion=Ok(());
                        if std::env::var_os("FILAVIM_CURSOR_SMOKE").is_some() {
                            for tick in 0..40 {
                                match window.update(cx,|root,w,cx|root.with_editor(w,cx,|v,w,cx|v.smoke_cursor_motion(tick,w,cx))) {
                                    Ok(Ok(()))=>{}, other=>{motion=Err(format!("{other:?}"));break;}
                                }
                                executor.timer(std::time::Duration::from_millis(25)).await;
                            }
                            executor.timer(std::time::Duration::from_millis(650)).await;
                        }
                        let first = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, _, cx| view.smoke_summary(cx)))
                            .unwrap_or_else(|e| format!("window error: {e}"));
                        if std::env::var_os("FILAVIM_CURSOR_SMOKE").is_some() {
                            let p=report.clone();let marker=format!("cursor_idle_begin\n{first}");
                            let _=executor.spawn(async move{std::fs::write(p,marker)}).await;
                        }
                        executor.timer(std::time::Duration::from_secs(2)).await;
                        let second = window.update(cx,|root,w,cx|root.with_editor(w,cx,|view, _, cx| view.smoke_summary(cx)))
                            .unwrap_or_else(|e| format!("window error: {e}"));
                        let text = format!("cursor_motion={motion:?}\ninput_and_undo={exercised:?}\nresize_and_cursor_geometry={geometry:?}\nfloating_picker={popup:?}\npicker_ready={popup_ready:?}\nfloating_confirmation={confirmation:?}\nfloating_information={information:?}\nproject_and_save={project:?}\nproject_ready={project_ready:?}\nafter settling: {first}after idle: {second}");
                        let _ = executor.spawn(async move { std::fs::write(report, text) }).await;
                        let _ = cx.update(|cx| cx.quit());
                    })
                    .detach();
                }
            }
        }
        cx.activate(true);
    });
    let error = result.borrow_mut().take();
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

/// Start the platform move loop only after a real drag, not a title-bar click.
pub(super) fn drag_window(window: &Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::{
            Input::KeyboardAndMouse::ReleaseCapture,
            WindowsAndMessaging::{PostMessageW, SC_MOVE, WM_SYSCOMMAND},
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        // Post rather than Send: do not reenter GPUI while its input state is borrowed.
        unsafe {
            ReleaseCapture();
            PostMessageW(
                handle.hwnd.get() as _,
                WM_SYSCOMMAND,
                (SC_MOVE | 2) as usize,
                0,
            );
        }
    }
    #[cfg(not(windows))]
    window.start_window_move();
}
