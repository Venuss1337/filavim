//! Render the actual widgets to portable SVGs without entering terminal mode.
//! Run with `cargo run --example snapshots`; output is in target/preview/.
use filavim::{
    animation::Visuals,
    app::{App, PickerKind},
    buffer::Buffer,
    config::Config,
    editor::Editor,
    input::Action,
    storage::History,
    syntax::SyntaxService,
    theme::Theme,
    ui,
    workspace::Workspace,
    workspace_ui,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use ropey::Rope;
use std::{
    collections::BTreeMap,
    fmt::Write,
    path::Path,
    time::{Duration, Instant},
};

fn color(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#232136".into(),
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn snapshot(
    name: &str,
    editor: &mut Editor,
    highlights: &[filavim::syntax::Highlight],
) -> anyhow::Result<()> {
    let mut terminal = Terminal::new(TestBackend::new(100, 32))?;
    terminal.draw(|f| ui::draw(f, editor, &Theme::default(), highlights, ""))?;
    export(name, &terminal)
}
fn export(name: &str, terminal: &Terminal<TestBackend>) -> anyhow::Result<()> {
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1100\" height=\"768\" viewBox=\"0 0 1100 768\">\n",
    );
    let buffer = terminal.backend().buffer();
    for y in 0..32 {
        for x in 0..100 {
            let cell = &buffer[(x, y)];
            writeln!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"11\" height=\"24\" fill=\"{}\"/>",
                x * 11,
                y * 24,
                color(cell.bg)
            )?;
        }
    }
    svg.push_str("<g font-family=\"Cascadia Code,Consolas,monospace\" font-size=\"18\" xml:space=\"preserve\">\n");
    for y in 0..32 {
        for x in 0..100 {
            let cell = &buffer[(x, y)];
            if cell.symbol() != " " {
                writeln!(
                    svg,
                    "<text x=\"{}\" y=\"{}\" fill=\"{}\">{}</text>",
                    x * 11,
                    y * 24 + 18,
                    color(cell.fg),
                    escape(cell.symbol())
                )?;
            }
        }
    }
    svg.push_str("</g></svg>");
    std::fs::write(format!("target/preview/{name}.svg"), svg)?;
    Ok(())
}
fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all("target/preview")?;
    let mut editor = Editor::new(Config::default());
    snapshot("dashboard", &mut editor, &[])?;
    let source = "//! A quiet place to build something.\n\nuse std::time::Instant;\n\nstruct Editor {\n    name: String,\n    responsive: bool,\n}\n\nfn main() {\n    let started = Instant::now();\n    let editor = Editor {\n        name: String::from(\"Filavim\"),\n        responsive: true,\n    };\n\n    // Native Rust. Thoughtful defaults.\n    println!(\"Welcome to {}\", editor.name);\n    println!(\"Ready in {:?}\", started.elapsed());\n}\n";
    editor.set_buffer(Buffer::new(
        Rope::from_str(source),
        Some("src/main.rs".into()),
    ));
    editor.buffer.cursor = editor.buffer.text.line_to_char(11) + 8;
    editor.notify("I insert  ·  WASD move  ·  :w save  ·  F1 help", false);
    let service = SyntaxService::new();
    service.request(
        editor.document_id,
        editor.buffer.revision,
        editor.buffer.text.clone(),
        Some(Path::new("src/main.rs")),
    );
    let syntax = service.results.recv_timeout(Duration::from_secs(5))?;
    snapshot("editor", &mut editor, &syntax.highlights)?;
    let fixture = Path::new("target/preview/project");
    std::fs::create_dir_all(fixture.join("src"))?;
    std::fs::write(fixture.join("src/main.rs"), source)?;
    std::fs::write(
        fixture.join("src/lib.rs"),
        "//! Filavim workspace\n\npub struct View {\n    pub buffer: u64,\n    pub cursor: usize,\n}\n\nimpl View {\n    pub fn new(buffer: u64) -> Self {\n        Self { buffer, cursor: 0 }\n    }\n}\n",
    )?;
    std::fs::write(
        fixture.join("README.md"),
        "# Filavim\n\nA quiet place to build something.\nNative Rust. Thoughtful defaults.\n",
    )?;
    let root = std::fs::canonicalize(fixture)?;
    editor.buffer.path = Some(root.join("src/main.rs"));
    let workspace = Workspace::new(editor, root.clone());
    let mut app = App::with_history_path(
        workspace,
        History::default(),
        root.join(".state/history.toml"),
    );
    wait(&mut app, |a| !a.pending())?;
    app.act(Action::Explorer);
    wait(&mut app, |a| !a.explorer.busy)?;
    app.explorer.selected = app
        .explorer
        .rows
        .iter()
        .position(|r| r.entry.label == "src")
        .unwrap();
    app.act(Action::Submit);
    wait(&mut app, |a| !a.explorer.busy)?;
    app.explorer.focused = false;
    app.act(Action::SplitVertical);
    app.command("e src/lib.rs");
    wait(&mut app, |a| !a.pending())?;
    app.workspace.new_tab();
    app.workspace.switch_tab(0);
    let mut spans = BTreeMap::from([(1, syntax.highlights)]);
    let ed = app.workspace.editor();
    service.request(
        2,
        ed.buffer.revision,
        ed.buffer.text.clone(),
        ed.buffer.path.as_deref(),
    );
    let parsed = service.results.recv_timeout(Duration::from_secs(5))?;
    spans.insert(2, parsed.highlights);
    let mut visuals = Visuals::new(app.workspace.config.effects());
    render_app("workspace", &mut app, &spans, &mut visuals)?;
    app.open_picker(PickerKind::Files);
    app.type_text("main");
    wait(&mut app, |a| {
        a.picker
            .as_ref()
            .is_some_and(|p| !p.busy && !p.preview.lines.is_empty())
    })?;
    render_app("files", &mut app, &spans, &mut visuals)?;
    app.act(Action::Normal);
    app.open_picker(PickerKind::Grep);
    app.type_text("pub");
    wait(&mut app, |a| {
        a.picker
            .as_ref()
            .is_some_and(|p| !p.busy && !p.preview.lines.is_empty())
    })?;
    render_app("grep", &mut app, &spans, &mut visuals)?;
    app.act(Action::Normal);
    app.open_picker(PickerKind::Palette);
    app.type_text("split");
    wait(&mut app, |a| {
        a.picker
            .as_ref()
            .is_some_and(|p| !p.busy && !p.preview.lines.is_empty())
    })?;
    render_app("palette", &mut app, &spans, &mut visuals)?;
    app.act(Action::Normal);
    app.command("qa!");
    wait(&mut app, App::ready_to_exit)?;
    println!(
        "Rendered dashboard, editor, workspace, files, grep, and palette SVGs in target/preview/"
    );
    Ok(())
}
fn wait(app: &mut App, condition: impl Fn(&App) -> bool) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.poll();
        if condition(app) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    anyhow::bail!("Snapshot worker timed out")
}
fn render_app(
    name: &str,
    app: &mut App,
    spans: &BTreeMap<u64, Vec<filavim::syntax::Highlight>>,
    visuals: &mut Visuals,
) -> anyhow::Result<()> {
    let mut terminal = Terminal::new(TestBackend::new(100, 32))?;
    terminal.draw(|f| {
        workspace_ui::draw(
            f,
            app,
            &Theme::default(),
            spans,
            visuals,
            "",
            Instant::now(),
        )
    })?;
    export(name, &terminal)
}
