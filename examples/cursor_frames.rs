//! Deterministic actual-widget cursor sequence; no interactive terminal required.
//! cargo run --example cursor_frames
use filavim::{
    animation::Visuals,
    app::App,
    buffer::Buffer,
    config::{Config, Preset},
    editor::Editor,
    input::Action,
    storage::History,
    theme::Theme,
    workspace::Workspace,
    workspace_ui,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use ropey::Rope;
use std::{
    collections::BTreeMap,
    fmt::Write,
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
}
fn main() -> anyhow::Result<()> {
    let dir = "target/cursor-preview";
    std::fs::create_dir_all(dir)?;
    let mut e = Editor::new(Config {
        preset: Preset::Beautiful,
        ..Config::default()
    });
    let source = (0..90)
        .map(|i| format!("    let item_{i:02} = build_editor(\"Rosé Pine Moon\"); // 日本 🦀\n"))
        .collect::<String>();
    e.set_buffer(Buffer::new(
        Rope::from_str(&source),
        Some("cursor-demo.rs".into()),
    ));
    let root = std::env::current_dir()?;
    let mut app = App::with_history_path(
        Workspace::new(e, root.clone()),
        History::default(),
        root.join(dir).join("history.toml"),
    );
    let mut visuals = Visuals::new(app.workspace.config.effects());
    let mut terminal = Terminal::new(TestBackend::new(90, 26))?;
    let now = Instant::now();
    let mut labels = Vec::new();
    for frame in 0..210 {
        let label = match frame {
            0..=19 => "Short horizontal motion",
            20..=49 => "Long horizontal jump",
            50..=89 => "Diagonal retargeting",
            90..=129 => "Scrolling / multi-line jump",
            130..=159 => "Insert: native cursor only",
            160..=179 => "Normal: no synthetic transition",
            _ => "Normal navigation resumes",
        };
        match frame {
            5 | 10 | 15 => app.act(Action::Right),
            20 => app.act(Action::End),
            50 => {
                for _ in 0..8 {
                    app.act(Action::Down);
                }
                app.act(Action::Home);
            }
            56 | 62 | 68 => {
                app.act(Action::Right);
                app.act(Action::Down);
            }
            90 => app.act(Action::Bottom),
            130 => app.act(Action::Insert),
            135 => app.type_text(" instant typing"),
            140 => app.act(Action::Backspace),
            145 => app.act(Action::Newline),
            160 => app.act(Action::Normal),
            180 => app.act(Action::Top),
            _ => {}
        }
        terminal.draw(|f| {
            workspace_ui::draw(
                f,
                &mut app,
                &Theme::default(),
                &BTreeMap::new(),
                &mut visuals,
                "",
                now + Duration::from_millis(frame * 17),
            )
        })?;
        let mut svg = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"990\" height=\"624\"><rect width=\"100%\" height=\"100%\" fill=\"#232136\"/><g font-family=\"Cascadia Code,Consolas,monospace\" font-size=\"17\" xml:space=\"preserve\">",
        );
        for y in 0..26 {
            for x in 0..90 {
                let c = &terminal.backend().buffer()[(x, y)];
                if c.bg != Theme::default().base {
                    write!(
                        svg,
                        "<rect x=\"{}\" y=\"{}\" width=\"11\" height=\"24\" fill=\"{}\"/>",
                        x * 11,
                        y * 24,
                        color(c.bg)
                    )?;
                }
                if c.symbol() != " " {
                    write!(
                        svg,
                        "<text x=\"{}\" y=\"{}\" fill=\"{}\">{}</text>",
                        x * 11,
                        y * 24 + 18,
                        color(c.fg),
                        escape(c.symbol())
                    )?;
                }
            }
        }
        if terminal.backend().cursor_visible() {
            let p = terminal.backend().cursor_position();
            write!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"24\" fill=\"#c4a7e7\" opacity=\"0.7\"/>",
                p.x * 11,
                p.y * 24,
                if app.mode() == filavim::editor::Mode::Insert {
                    2
                } else {
                    11
                }
            )?;
        }
        svg.push_str("</g></svg>");
        std::fs::write(format!("{dir}/{frame:03}.svg"), svg)?;
        labels.push(label);
    }
    let labels = labels
        .iter()
        .map(|l| format!("\"{l}\""))
        .collect::<Vec<_>>()
        .join(",");
    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><title>Filavim spring cursor review</title><style>body{{background:#232136;color:#e0def4;font:16px monospace;margin:24px}}img{{max-width:100%;display:block}}input{{width:90%}}button{{padding:8px}}</style><h2>Filavim — deterministic cursor frames</h2><p id="label"></p><img id="image"><p><button id="play">Play / pause</button> <span id="frame"></span></p><input id="seek" type="range" min="0" max="209" value="0"><script>const labels=[{labels}];let i=0,playing=true;const image=document.querySelector('#image'),seek=document.querySelector('#seek');for(let n=0;n<210;n++){{let p=new Image();p.src=String(n).padStart(3,'0')+'.svg'}}function show(){{image.src=String(i).padStart(3,'0')+'.svg';document.querySelector('#label').textContent=labels[i];document.querySelector('#frame').textContent=i+' / 209';seek.value=i}}document.querySelector('#play').onclick=()=>playing=!playing;seek.oninput=()=>{{playing=false;i=+seek.value;show()}};setInterval(()=>{{if(playing){{i=(i+1)%210;show()}}}},17);show();</script>"#
    );
    std::fs::write(format!("{dir}/index.html"), html)?;
    println!("Open {dir}/index.html to review 210 actual-widget frames.");
    Ok(())
}
