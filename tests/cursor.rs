use filavim::{
    animation::{CursorTarget, Visuals},
    config::{Config, Preset},
    editor::Mode,
    theme::Theme,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Style, widgets::Block};
use std::time::{Duration, Instant};

fn target(x: u16, y: u16, logical: usize) -> CursorTarget {
    CursorTarget {
        position: (x, y),
        viewport: Rect::new(3, 2, 32, 10),
        origin: (0, 0),
        logical,
        revision: 0,
    }
}
fn draw(t: &mut Terminal<TestBackend>, v: &mut Visuals, target: CursorTarget, now: Instant) {
    t.draw(|f| {
        f.render_widget(
            Block::default().style(Style::default().bg(Theme::default().base)),
            f.area(),
        );
        v.cursor(Some(target), now, &Theme::default(), f);
    })
    .unwrap();
}
#[test]
fn native_cursor_is_hidden_only_during_normal_navigation_and_restored() {
    let mut t = Terminal::new(TestBackend::new(40, 15)).unwrap();
    let mut v = Visuals::new(Config::default().effects());
    let now = Instant::now();
    v.context(1, 1, Mode::Normal as u8, Rect::new(0, 0, 40, 15));
    draw(&mut t, &mut v, target(4, 3, 0), now);
    assert!(t.backend().cursor_visible());
    draw(&mut t, &mut v, target(25, 8, 50), now);
    assert!(!t.backend().cursor_visible());
    assert!(v.active(now));
    for frame in 1..120 {
        draw(
            &mut t,
            &mut v,
            target(25, 8, 50),
            now + Duration::from_millis(frame * 34),
        );
    }
    assert!(!v.active(now));
    assert!(t.backend().cursor_visible());
    assert_eq!(t.backend().cursor_position(), (25, 8).into());
    assert!(
        t.backend()
            .buffer()
            .content
            .iter()
            .all(|c| c.symbol() == " " && c.bg == Theme::default().base)
    );
}
#[test]
fn insert_visual_command_cancel_and_returning_normal_seeds_without_motion() {
    for mode in [Mode::Insert, Mode::Visual, Mode::Command] {
        let mut t = Terminal::new(TestBackend::new(40, 15)).unwrap();
        let now = Instant::now();
        let mut v = Visuals::new(Config::default().effects());
        v.context(1, 1, Mode::Normal as u8, Rect::new(0, 0, 40, 15));
        draw(&mut t, &mut v, target(4, 3, 0), now);
        draw(&mut t, &mut v, target(25, 8, 50), now);
        assert!(v.active(now));
        v.context(1, 1, mode as u8, Rect::new(0, 0, 40, 15));
        for i in 0..10 {
            let mut p = target(4 + i, 3, usize::from(i));
            p.revision = u64::from(i);
            draw(&mut t, &mut v, p, now);
            assert!(!v.active(now));
            assert!(t.backend().cursor_visible());
        }
        v.context(1, 1, Mode::Normal as u8, Rect::new(0, 0, 40, 15));
        draw(&mut t, &mut v, target(20, 5, 100), now);
        assert!(!v.active(now));
        assert!(t.backend().cursor_visible());
    }
}
#[test]
fn edits_and_viewport_only_motion_do_not_create_smears() {
    let mut t = Terminal::new(TestBackend::new(40, 15)).unwrap();
    let now = Instant::now();
    let mut v = Visuals::new(Config::default().effects());
    v.context(1, 1, 0, Rect::new(0, 0, 40, 15));
    draw(&mut t, &mut v, target(4, 3, 0), now);
    let mut p = target(7, 5, 12);
    p.revision = 1;
    draw(&mut t, &mut v, p, now);
    assert!(!v.active(now));
    p.position = (7, 4);
    p.origin = (0, 1);
    draw(&mut t, &mut v, p, now);
    assert!(!v.active(now));
    p.logical = 100;
    p.position = (20, 8);
    draw(&mut t, &mut v, p, now);
    assert!(v.active(now));
    p.origin = (0, 3);
    p.position = (20, 6);
    draw(&mut t, &mut v, p, now + Duration::from_millis(34));
    assert!(v.active(now));
    v.context(2, 1, 0, Rect::new(0, 0, 20, 15));
    assert!(!v.active(now));
}
#[test]
fn minimal_disabled_and_native_fallback_never_schedule_cursor_frames() {
    for text in [
        "preset='minimal'",
        "[visual]\ncursor_animation=false",
        "[visual.smear]\nquality='native'",
    ] {
        let c = Config::parse(text).unwrap();
        let mut v = Visuals::new(c.effects());
        let mut t = Terminal::new(TestBackend::new(40, 15)).unwrap();
        let now = Instant::now();
        v.context(1, 1, 0, Rect::new(0, 0, 40, 15));
        draw(&mut t, &mut v, target(4, 3, 0), now);
        draw(&mut t, &mut v, target(25, 8, 50), now);
        assert!(!v.active(now));
        assert!(t.backend().cursor_visible());
        if c.preset == Preset::Minimal {
            assert_eq!(v.scroll(1, 5, 10, 20, now), 5);
            assert!(!v.active(now));
        }
    }
}
#[test]
fn config_validates_physics_and_preserves_individual_overrides() {
    for text in [
        "stiffness=0.0",
        "trailing_stiffness=1.0",
        "damping=nan",
        "anticipation=-1.0",
        "max_length=0.0",
        "frame_interval_ms=0",
        "color='red'",
        "quality='unknown'",
    ] {
        assert!(
            Config::parse(&format!("[visual.smear]\n{text}")).is_err(),
            "{text}"
        );
    }
    let c=Config::parse("preset='minimal'\n[visual]\ncursor_animation=true\n[visual.smear]\ncolor='#eb6f92'\nframe_interval_ms=17\nstiffness=0.7\nquality='ascii'").unwrap();
    assert!(c.effects().cursor);
    assert!(!c.effects().scroll);
    assert_eq!(c.effects().smear.stiffness, 0.7);
    assert_eq!(
        Visuals::new(c.effects()).frame_interval(),
        Duration::from_millis(17)
    );
    Config::parse(include_str!("../config.example.toml")).unwrap();
    for fps in [60, 120, 240] {
        let config = Config::parse(&format!("[visual]\nfps={fps}")).unwrap();
        assert_eq!(config.effects().fps, fps);
    }
    assert!(Config::parse("[visual]\nfps=241").is_err());
    assert!(Config::parse("[visual.smear]\nframe_interval_ms=3").is_err());
    assert_eq!(Config::default().effects().fps, 60);
}

#[test]
fn cached_frames_match_full_widgets_and_only_change_the_cursor_region() {
    use filavim::{
        app::App,
        buffer::Buffer,
        editor::Editor,
        input::Action,
        storage::History,
        syntax::{Highlight, Token},
        workspace::Workspace,
        workspace_ui,
    };
    use ropey::Rope;
    use std::collections::BTreeMap;
    use unicode_width::UnicodeWidthStr;
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let mut editor = Editor::new(config.clone());
    let source = "    let value = render_editor(\"日本 🦀 Rosé Pine Moon\");\n".repeat(20);
    editor.set_buffer(Buffer::new(Rope::from_str(&source), None));
    let mut app = App::with_history_path(
        Workspace::new(editor, root.path().into()),
        History::default(),
        root.path().join("history.toml"),
    );
    let spans = BTreeMap::from([(
        app.workspace.active_buffer(),
        vec![Highlight {
            range: 0..source.len(),
            token: Token::String,
        }],
    )]);
    let mut full = Terminal::new(TestBackend::new(100, 25)).unwrap();
    let mut cached = Terminal::new(TestBackend::new(100, 25)).unwrap();
    let mut full_visuals = Visuals::new(config.effects());
    let mut cached_visuals = Visuals::new(config.effects());
    let now = Instant::now();
    let theme = Theme::default();
    full.draw(|f| workspace_ui::draw(f, &mut app, &theme, &spans, &mut full_visuals, "", now))
        .unwrap();
    cached
        .draw(|f| workspace_ui::draw(f, &mut app, &theme, &spans, &mut cached_visuals, "", now))
        .unwrap();
    app.act(Action::End);
    for frame in 0..50 {
        let time = now + Duration::from_nanos(16_666_667 * (frame + 1));
        let before = cached.backend().buffer().clone();
        full.draw(|f| workspace_ui::draw(f, &mut app, &theme, &spans, &mut full_visuals, "", time))
            .unwrap();
        cached
            .draw(|f| {
                if frame == 0 {
                    workspace_ui::draw(f, &mut app, &theme, &spans, &mut cached_visuals, "", time);
                } else {
                    assert!(cached_visuals.redraw_cursor(f, time, &theme));
                }
            })
            .unwrap();
        for y in 0..25 {
            let mut x = 0;
            while x < 100 {
                let expected = &full.backend().buffer()[(x, y)];
                assert_eq!(
                    &cached.backend().buffer()[(x, y)],
                    expected,
                    "frame {frame}, cell {x},{y}"
                );
                x += expected.symbol().width().max(1) as u16;
            }
        }
        assert_eq!(
            full.backend().cursor_visible(),
            cached.backend().cursor_visible()
        );
        if frame > 0 {
            assert!(before.diff(cached.backend().buffer()).len() < 250);
        }
    }
    assert!(!cached_visuals.active(now));
    app.act(Action::Insert);
    cached
        .draw(|f| {
            workspace_ui::draw(
                f,
                &mut app,
                &theme,
                &spans,
                &mut cached_visuals,
                "",
                now + Duration::from_secs(1),
            )
        })
        .unwrap();
    cached
        .draw(|f| assert!(!cached_visuals.redraw_cursor(f, now + Duration::from_secs(1), &theme)))
        .unwrap();
}

#[test]
fn workspace_unicode_splits_and_mode_changes_preserve_the_editor() {
    use filavim::{
        app::App, buffer::Buffer, editor::Editor, input::Action, storage::History,
        workspace::Workspace, workspace_ui,
    };
    use ropey::Rope;
    use std::collections::BTreeMap;
    let root = tempfile::tempdir().unwrap();
    let c = Config {
        preset: Preset::Beautiful,
        ..Config::default()
    };
    let mut e = Editor::new(c.clone());
    e.set_buffer(Buffer::new(
        Rope::from_str(&"日本 🦀 e\u{301}\twide text\n".repeat(80)),
        None,
    ));
    let mut app = App::with_history_path(
        Workspace::new(e, root.path().into()),
        History::default(),
        root.path().join("history.toml"),
    );
    app.act(Action::SplitVertical);
    let mut t = Terminal::new(TestBackend::new(100, 25)).unwrap();
    let mut v = Visuals::new(c.effects());
    let now = Instant::now();
    let render = |t: &mut Terminal<TestBackend>, app: &mut App, v: &mut Visuals, time| {
        t.draw(|f| workspace_ui::draw(f, app, &Theme::default(), &BTreeMap::new(), v, "", time))
            .unwrap();
    };
    render(&mut t, &mut app, &mut v, now);
    let baseline = t.backend().buffer().clone();
    app.act(Action::Right);
    app.act(Action::Down);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(17));
    assert!(!t.backend().cursor_visible());
    // The inactive left pane, its border and gutter are never painted by smear.
    for y in 2..23 {
        for x in 0..50 {
            assert_eq!(t.backend().buffer()[(x, y)], baseline[(x, y)]);
        }
    }
    for frame in 2..50 {
        render(
            &mut t,
            &mut app,
            &mut v,
            now + Duration::from_millis(frame * 17),
        );
    }
    assert!(!v.active(now));
    let restored = t.backend().buffer().clone();
    let mut fresh = Terminal::new(TestBackend::new(100, 25)).unwrap();
    let mut effects = c.effects();
    effects.cursor = false;
    effects.scroll = false;
    render(
        &mut fresh,
        &mut app,
        &mut Visuals::new(effects),
        now + Duration::from_secs(1),
    );
    // TestBackend retains hidden continuation cells; real terminals display the
    // leading wide glyph over them. Compare every visible cell and its styles.
    use unicode_width::UnicodeWidthStr;
    for y in 0..25 {
        let mut x = 0;
        while x < 100 {
            let expected = &fresh.backend().buffer()[(x, y)];
            assert_eq!(
                &restored[(x, y)],
                expected,
                "visible cell {x},{y} must restore"
            );
            x += expected.symbol().width().max(1) as u16;
        }
    }
    let now = now + Duration::from_secs(1);
    app.act(Action::Insert);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(18));
    assert!(!v.active(now));
    assert!(t.backend().cursor_visible());
    app.type_text("instant");
    app.act(Action::Newline);
    app.act(Action::Backspace);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(19));
    assert!(!v.active(now));
    app.act(Action::Normal);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(20));
    assert!(!v.active(now));
    app.act(Action::Bottom);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(40));
    t.backend_mut().resize(12, 6);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(60));
    t.backend_mut().resize(8, 3);
    render(&mut t, &mut app, &mut v, now + Duration::from_millis(80));
    assert!(!v.active(now));
}
