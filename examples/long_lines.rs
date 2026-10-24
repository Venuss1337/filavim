//! Individual headless samples, including hidden horizontal prefixes.
use filavim::{
    animation::Visuals,
    app::App,
    buffer::Buffer,
    config::{Config, Preset},
    editor::Editor,
    input::Action,
    storage::History,
    theme::Theme,
    ui,
    workspace::{Axis, Workspace},
    workspace_ui,
};
use ratatui::{Terminal, backend::TestBackend};
use ropey::Rope;
use std::time::Instant;

fn report(name: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100) - 1];
    println!(
        "{name}: n={} p50_us={:.2} p95_us={:.2} p99_us={:.2}",
        samples.len(),
        percentile(50),
        percentile(95),
        percentile(99)
    );
}
fn main() {
    let theme = Theme::default();
    for (name, unit) in [
        ("ascii", "x"),
        ("tabs", "x\t"),
        ("combining", "e\u{301}"),
        ("emoji", "👩‍💻"),
    ] {
        let text = unit.repeat(1_000_000 / unit.len());
        for (position, fraction) in [("start", 0), ("middle", 1), ("end", 2)] {
            let mut editor = Editor::new(Config::default());
            editor.set_buffer(Buffer::new(Rope::from_str(&text), None));
            editor.buffer.cursor = editor.buffer.text.len_chars() * fraction / 2;
            editor.buffer.snap_cursor();
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            let cold = Instant::now();
            terminal
                .draw(|f| ui::draw(f, &mut editor, &theme, &[], ""))
                .unwrap();
            println!(
                "{name}/{position}: cold_us={:.2}",
                cold.elapsed().as_secs_f64() * 1e6
            );
            let mut samples = Vec::new();
            for i in 0..100 {
                let at = Instant::now();
                editor
                    .buffer
                    .horizontal(i % 2 == 0 && fraction != 2 || i % 2 == 1 && fraction == 2);
                terminal
                    .draw(|f| ui::draw(f, &mut editor, &theme, &[], ""))
                    .unwrap();
                samples.push(at.elapsed().as_secs_f64() * 1e6);
            }
            report(&format!("{name}/{position}/navigation_frame"), samples);
            let mut samples = Vec::new();
            for i in 0..40 {
                let at = Instant::now();
                let cursor = editor.buffer.cursor;
                let at_char = if i % 2 == 0 {
                    cursor
                } else {
                    cursor.saturating_sub(32)
                };
                editor.buffer.replace(at_char..at_char, "z");
                terminal
                    .draw(|f| ui::draw(f, &mut editor, &theme, &[], ""))
                    .unwrap();
                samples.push(at.elapsed().as_secs_f64() * 1e6);
            }
            report(&format!("{name}/{position}/edit_frame"), samples);
            let mut samples = Vec::new();
            for _ in 0..20 {
                let cursor = editor.buffer.cursor;
                let at = Instant::now();
                editor.buffer.replace(0..0, "z");
                editor.buffer.cursor = cursor + 1;
                editor.buffer.snap_cursor();
                terminal
                    .draw(|f| ui::draw(f, &mut editor, &theme, &[], ""))
                    .unwrap();
                samples.push(at.elapsed().as_secs_f64() * 1e6);
            }
            report(
                &format!("{name}/{position}/prefix_invalidation_frame"),
                samples,
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.git.enabled = false;
        config.lsp.enabled = false;
        config.syntax = false;
        config.preset = Preset::Minimal;
        let mut editor = Editor::new(config.clone());
        editor.set_buffer(Buffer::new(Rope::from_str(&text), None));
        let mut workspace = Workspace::new(editor, dir.path().into());
        let first = workspace.focused();
        workspace.split(Axis::Vertical);
        workspace.split(Axis::Horizontal);
        workspace.switch_view(first);
        workspace.split(Axis::Horizontal);
        let len = workspace.editor().buffer.text.len_chars();
        for (i, view) in workspace.views.values_mut().enumerate() {
            view.cursor = len * (i + 1) / 4;
        }
        workspace.load(workspace.focused());
        let mut app =
            App::with_history_path(workspace, History::default(), dir.path().join("history"));
        let mut visuals = Visuals::new(config.effects());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let highlights = Default::default();
        terminal
            .draw(|f| {
                workspace_ui::draw(
                    f,
                    &mut app,
                    &theme,
                    &highlights,
                    &mut visuals,
                    "",
                    Instant::now(),
                )
            })
            .unwrap();
        let mut samples = Vec::new();
        for i in 0..100 {
            let at = Instant::now();
            app.workspace.apply(if i % 2 == 0 {
                Action::Left
            } else {
                Action::Right
            });
            terminal
                .draw(|f| {
                    workspace_ui::draw(
                        f,
                        &mut app,
                        &theme,
                        &highlights,
                        &mut visuals,
                        "",
                        Instant::now(),
                    )
                })
                .unwrap();
            samples.push(at.elapsed().as_secs_f64() * 1e6);
        }
        report(&format!("{name}/four_splits/navigation_frame"), samples);
    }
}
