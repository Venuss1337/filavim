use super::{FONT_BYTES, model, view};
use gpui::{prelude::*, *};
use std::{borrow::Cow, path::PathBuf};
pub fn run(path: Option<PathBuf>, config: crate::config::Config) -> anyhow::Result<()> {
    run_inner(path, config, None)
}
/// Bounded native startup/paint/idle smoke test; never edits or saves the fixture.
pub fn smoke(path: PathBuf, report: PathBuf) -> anyhow::Result<()> {
    std::fs::write(&report, "starting GPUI\n")?;
    run_inner(
        Some(path),
        crate::config::Config::default(),
        Some(report.clone()),
    )?;
    let text = std::fs::read_to_string(report)?;
    anyhow::ensure!(
        text.contains("input_and_undo=Ok(Ok(()))")
            && text.contains("resize_and_cursor_geometry=Ok(Ok(()))"),
        "native smoke checks failed: {text}"
    );
    Ok(())
}
fn run_inner(
    path: Option<PathBuf>,
    config: crate::config::Config,
    report: Option<PathBuf>,
) -> anyhow::Result<()> {
    let model = model::Model::new(config)?;
    let failure = std::rc::Rc::new(std::cell::RefCell::new(None));
    let result = failure.clone();
    Application::new().run(move |cx: &mut App| {
        if let Some(p) = &report {
            let _ = std::fs::write(p, "application callback\n");
        }
        if let Err(e) = cx.text_system().add_fonts(vec![Cow::Borrowed(FONT_BYTES)]) {
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
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(420.), px(240.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Filavim".into()),
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
                    if let Some(path) = path {
                        view.open_initial(path, cx);
                    }
                });
                editor
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
                    cx.spawn(async move |cx| {
                        executor.timer(std::time::Duration::from_secs(3)).await;
                        let exercised = window.update(cx, |view, window, cx| view.smoke_edit(window, cx));
                        executor.timer(std::time::Duration::from_millis(250)).await;
                        let geometry = window.update(cx, |view, window, cx| view.smoke_geometry(window, cx));
                        executor.timer(std::time::Duration::from_secs(2)).await;
                        let first = window.update(cx, |view, _, _| view.smoke_summary())
                            .unwrap_or_else(|e| format!("window error: {e}"));
                        executor.timer(std::time::Duration::from_secs(2)).await;
                        let second = window.update(cx, |view, _, _| view.smoke_summary())
                            .unwrap_or_else(|e| format!("window error: {e}"));
                        let text = format!("input_and_undo={exercised:?}\nresize_and_cursor_geometry={geometry:?}\nafter settling: {first}after idle: {second}");
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
