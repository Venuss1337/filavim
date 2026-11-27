use filavim::{
    buffer::Buffer,
    config::Config,
    gui::{
        documents::Documents,
        model::Model,
        popup::Presentation,
        services::{Kind, Services},
    },
    lsp::Encoding,
};
use ropey::Rope;
use std::{
    thread,
    time::{Duration, Instant},
};
fn documents() -> Documents {
    let mut config = Config::default();
    config.lsp.enabled = false;
    Documents::new(Model::new(config).unwrap())
}
fn wait(mut ready: impl FnMut() -> bool, seconds: u64) {
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if ready() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("language service deadline exceeded");
}
#[test]
fn completion_settings_are_validated_and_backward_compatible() {
    let c = Config::parse("").unwrap();
    assert!(c.completion.auto_trigger && c.completion.accept_enter && c.completion.accept_tab);
    let c = Config::parse("[completion]\nauto_trigger=false\naccept_enter=false\naccept_tab=true")
        .unwrap();
    assert!(!c.completion.auto_trigger && !c.completion.accept_enter && c.completion.accept_tab);
    assert!(Config::parse("[completion]\naccept_entr=false").is_err());
}
#[test]
fn contextual_diagnostics_include_multiple_severities_without_changing_buffer() {
    let mut d = documents();
    d.add(
        Buffer::new(Rope::from_str("let x = 1;\n"), Some("fixture.rs".into())),
        true,
    )
    .unwrap();
    let before = d.editor.buffer.text.clone();
    let mut s = Services::new(".".into());
    let range = lsp_types::Range::new(
        lsp_types::Position::new(0, 0),
        lsp_types::Position::new(0, 3),
    );
    s.diagnostics.insert(
        ("fixture".into(), d.id()),
        (
            0,
            Encoding::Utf16,
            vec![
                filavim::gui::services::Diagnostic {
                    range,
                    severity: 1,
                    message: "first error".into(),
                },
                filavim::gui::services::Diagnostic {
                    range,
                    severity: 2,
                    message: "second warning".into(),
                },
            ],
        ),
    );
    s.diagnostics_at(&d);
    let p = s.panel.as_ref().unwrap();
    assert_eq!(p.presentation, Presentation::Information);
    assert_eq!(p.rows.len(), 2);
    assert!(p.rows[0].label.contains("[Error]"));
    assert!(p.rows[1].label.contains("[Warning]"));
    s.dismiss();
    assert!(s.panel.is_none());
    assert_eq!(d.editor.buffer.text, before);
}
#[test]
#[ignore = "requires installed real rust-analyzer; run serially with --ignored --nocapture"]
fn real_rust_analyzer_completion_hover_and_diagnostics() {
    assert!(
        std::process::Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .is_ok(),
        "rust-analyzer is unavailable"
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("Cargo.toml"),"[package]\nname=\"filavim_popup_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    let source = "/// Widget documentation with Unicode é😀.\nstruct Widget { value: i32 }\nimpl Widget {\n    /// Computes the widget value.\n    fn compute(&self) -> i32 { self.value }\n}\nfn main() {\n    let widget = Widget { value: 7 };\n    let _answer = widget.com;\n    let _wrong = ;\n}\n";
    let path = temp.path().join("src/main.rs");
    std::fs::write(&path, source).unwrap();
    let mut config = Config::default();
    config.lsp.servers.retain(|s| s.name == "rust");
    config.lsp.servers[0].initialization_options = serde_json::json!({"checkOnSave":false,"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false}});
    let mut d = Documents::new(Model::new(config).unwrap());
    d.add(Buffer::new(Rope::from_str(source), Some(path)), true)
        .unwrap();
    d.editor.set_mode(filavim::editor::Mode::Insert);
    let byte = source.find("widget.com").unwrap() + "widget.com".len();
    d.editor.buffer.cursor = source[..byte].chars().count();
    let mut s = Services::new(temp.path().to_owned());
    s.update(&d);
    wait(
        || {
            s.poll(&mut d);
            s.completion_ready(&d)
        },
        30,
    );
    let mut last = Instant::now() - Duration::from_secs(2);
    wait(
        || {
            s.poll(&mut d);
            if s.panel.as_ref().is_some_and(|p| {
                p.kind == Kind::Completion && p.rows.iter().any(|r| r.label.starts_with("compute"))
            }) {
                return true;
            }
            if last.elapsed() > Duration::from_secs(1) {
                s.request(&d, Kind::Completion, None);
                last = Instant::now();
            }
            false
        },
        60,
    );
    let selected = s
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|r| r.label.starts_with("compute"))
        .unwrap();
    s.panel.as_mut().unwrap().selected = selected;
    s.selection_changed(&d);
    s.accept(&mut d);
    wait(
        || {
            s.poll(&mut d);
            d.editor.buffer.text.to_string().contains("widget.compute(")
        },
        15,
    );
    let edited = d.editor.buffer.text.to_string();
    assert!(!edited.contains("${"));
    assert_eq!(d.editor.mode, filavim::editor::Mode::Insert);
    println!(
        "REAL rust-analyzer accepted: {}",
        edited.lines().find(|l| l.contains("_answer")).unwrap()
    );
    let byte = edited.find("widget.compute").unwrap() + "widget.co".len();
    d.editor.buffer.cursor = edited[..byte].chars().count();
    s.update(&d);
    s.request(&d, Kind::Hover, None);
    wait(
        || {
            s.poll(&mut d);
            s.panel.as_ref().is_some_and(|p| p.kind == Kind::Hover)
        },
        15,
    );
    assert!(
        s.panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.label.contains("Computes"))
    );
    println!("REAL rust-analyzer hover documentation verified");
    let text = d.editor.buffer.text.to_string();
    let byte = text.find("_wrong = ;").unwrap() + "_wrong = ".len();
    d.editor.buffer.cursor = text[..byte].chars().count();
    d.editor.buffer.insert("(");
    s.update(&d);
    wait(
        || {
            s.poll(&mut d);
            s.diagnostics
                .values()
                .any(|(rev, _, items)| *rev == d.editor.buffer.revision && !items.is_empty())
        },
        30,
    );
    println!(
        "REAL rust-analyzer diagnostic count: {}",
        s.diagnostics
            .values()
            .map(|(_, _, d)| d.len())
            .sum::<usize>()
    );
    s.dismiss();
    assert!(s.panel.is_none());
    s.shutdown();
    wait(|| s.stopped(), 5);
}
