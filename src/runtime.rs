use crate::{
    animation::Visuals,
    app::{App, Effect},
    editor::Mode,
    input::{KeyEvents, Keymap, text_character},
    pixiedust::graphics::{CellSize, Graphics, ReplyFilter},
    syntax::{Highlight, SyntaxService, language_name},
    theme::Theme,
    workspace_ui,
};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

pub fn run<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    app: App,
    keymap: Keymap,
    theme: Theme,
) -> Result<()> {
    let mut graphics = Graphics::native(
        app.workspace.config.effects(),
        particle_color(&theme, app.workspace.config.effects()),
    );
    let (filter_enabled, version_pending, image_id) = graphics.filter();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let wake_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wake_flag = wake_pending.clone();
    let wake_tx = input_tx.clone();
    let _wake = crate::background::WakeGuard::install(std::sync::Arc::new(move || {
        if !wake_flag.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let _ = wake_tx.send(Incoming::Wake);
        }
    }));
    std::thread::spawn(move || {
        let mut filter: Option<ReplyFilter> = None;
        loop {
            if let Some(filter) = &mut filter
                && filter.holding()
            {
                match event::poll(Duration::from_millis(10)) {
                    Ok(false) => {
                        if filter.expired(Instant::now()) {
                            filter.flush(|e| {
                                let _ = input_tx.send(Incoming::input(Ok(e)));
                            });
                        }
                        continue;
                    }
                    Err(e) => {
                        let _ = input_tx.send(Incoming::input(Err(e)));
                        break;
                    }
                    _ => {}
                }
            }
            let event = event::read();
            if filter_enabled.load(std::sync::atomic::Ordering::Acquire)
                && let Ok(event) = event
            {
                let filter = filter.get_or_insert_with(|| ReplyFilter::new(image_id));
                filter.version_pending(version_pending.load(std::sync::atomic::Ordering::Acquire));
                let mut disconnected = false;
                filter.push(
                    event,
                    Instant::now(),
                    |e| {
                        disconnected |= input_tx.send(Incoming::input(Ok(e))).is_err();
                    },
                    |reply| {
                        if reply.starts_with(">|") {
                            version_pending.store(false, std::sync::atomic::Ordering::Release);
                        }
                        let _ = input_tx.send(Incoming::Graphics(reply));
                    },
                );
                if disconnected {
                    break;
                }
                continue;
            }
            let failed = event.is_err();
            if input_tx.send(Incoming::input(event)).is_err() || failed {
                break;
            }
        }
    });
    let result = run_loop(
        terminal,
        app,
        keymap,
        theme,
        input_rx,
        wake_pending,
        (
            |backend, mode, clipboard, graphics: &[u8]| {
                if let Some(mode) = mode {
                    execute!(
                        backend,
                        match mode {
                            Mode::Normal => SetCursorStyle::SteadyBlock,
                            Mode::Visual => SetCursorStyle::SteadyUnderScore,
                            _ => SetCursorStyle::SteadyBar,
                        }
                    )?;
                }
                if let Some(text) = clipboard {
                    write!(
                        backend,
                        "\x1b]52;c;{}\x07",
                        STANDARD.encode(text.as_bytes())
                    )?;
                    backend.flush()?;
                }
                if !graphics.is_empty() {
                    backend.write_all(graphics)?;
                    backend.flush()?;
                }
                Ok(())
            },
            &mut graphics,
        ),
    )
    .map(|_| ());
    let cleanup = terminal
        .backend_mut()
        .write_all(graphics.finish())
        .and_then(|_| terminal.backend_mut().flush());
    result.and(cleanup.map_err(Into::into))
}
fn particle_color(theme: &Theme, effects: crate::config::Effects) -> [u8; 3] {
    let (r, g, b) =
        crate::smear::rgb(effects.smear.color.unwrap_or(theme.iris)).unwrap_or((196, 167, 231));
    [r, g, b]
}

struct InputEvent {
    event: std::io::Result<Event>,
    #[cfg(feature = "perf")]
    enqueued: Instant,
}
enum Incoming {
    Input(InputEvent),
    Wake,
    Graphics(String),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PendingSyntax {
    document: u64,
    revision: u64,
    generation: u64,
}
type SyntaxVersion = (u64, u64, Option<PathBuf>, bool, usize);
fn complete_syntax_pending(
    pending: &mut BTreeMap<u64, PendingSyntax>,
    id: u64,
    stamp: PendingSyntax,
) -> bool {
    if pending.get(&id) == Some(&stamp) {
        pending.remove(&id);
        true
    } else {
        false
    }
}
fn retire_syntax_pending(pending: &mut BTreeMap<u64, PendingSyntax>, app: &App) {
    pending.retain(|id, stamp| {
        app.workspace
            .buffers
            .get(id)
            .is_some_and(|e| (stamp.document, stamp.revision) == (e.document_id, e.buffer.revision))
    });
}
impl Incoming {
    fn input(event: std::io::Result<Event>) -> Self {
        Self::Input(InputEvent {
            event,
            #[cfg(feature = "perf")]
            enqueued: crate::perf::enqueued(),
        })
    }
}

// Shared by the real input source and deterministic headless runtime tests.
fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    mut keymap: Keymap,
    mut theme: Theme,
    input_rx: std::sync::mpsc::Receiver<Incoming>,
    wake_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    (mut output, graphics): (
        impl FnMut(&mut B, Option<Mode>, Option<&str>, &[u8]) -> Result<()>,
        &mut Graphics,
    ),
) -> Result<App>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let syntax = SyntaxService::new();
    let mut spans: BTreeMap<u64, Vec<Highlight>> = BTreeMap::new();
    let mut snapshots: BTreeMap<u64, (u64, Vec<Highlight>)> = BTreeMap::new();
    let mut versions: BTreeMap<u64, SyntaxVersion> = BTreeMap::new();
    let mut syntax_pending = BTreeMap::new();
    let mut visuals = Visuals::new(app.workspace.config.effects());
    let mut dirty = true;
    let mut clock = FrameClock::new(visuals.frame_interval());
    let mut timer = AnimationTimer::default();
    let mut key_events = KeyEvents::default();
    let mut cursor_mode = None;
    let mut config_epoch = app.ide.config_epoch;
    let mut pixiedust_notice = false;
    let mut shutdown_deadline = None;
    loop {
        let background_timer = crate::perf::Timer::start();
        dirty |= app.poll();
        background_timer.finish("background_poll");
        if config_epoch != app.ide.config_epoch {
            pixiedust_notice = false;
            config_epoch = app.ide.config_epoch;
            keymap = Keymap::new(&app.workspace.config)?;
            theme = Theme::from_config(&app.workspace.config)?
                .for_terminal(app.workspace.config.visual.colors);
            visuals = Visuals::new(app.workspace.config.effects());
            graphics.configure(visuals.effects, particle_color(&theme, visuals.effects));
            clock = FrameClock::new(visuals.frame_interval());
            dirty = true;
        }
        let commands = graphics.service(Instant::now());
        if !commands.is_empty() {
            output(terminal.backend_mut(), None, None, commands)?;
        }
        if visuals.pixel_cell(graphics.ready()) {
            dirty = true;
            clock = FrameClock::new(visuals.frame_interval());
        }
        if !pixiedust_notice
            && visuals.effects.cursor
            && visuals.effects.cursor_effect == crate::config::CursorEffect::Pixiedust
            && graphics.fallback()
        {
            app.notify(
                "Pixiedust unavailable or suspended; using the smear cursor",
                false,
            );
            pixiedust_notice = true;
            dirty = true;
        }
        if app.ready_to_exit() {
            let deadline = shutdown_deadline.get_or_insert_with(|| {
                app.ide.shutdown();
                Instant::now() + Duration::from_secs(2)
            });
            if app.ide.stopped() || Instant::now() >= *deadline {
                return Ok(app);
            }
        }
        if dirty {
            let mut visible = Vec::new();
            app.workspace.tabs[app.workspace.active_tab]
                .layout
                .leaves(&mut visible);
            let ids: BTreeSet<_> = visible
                .into_iter()
                .map(|v| app.workspace.views[&v].buffer)
                .collect();
            for id in ids {
                let editor = &app.workspace.buffers[&id];
                let version = (
                    editor.buffer.revision,
                    editor.document_id,
                    editor.buffer.path.clone(),
                    editor.config.syntax,
                    editor.config.syntax_max_bytes,
                );
                if versions.get(&id) != Some(&version) {
                    let old = versions.insert(id, version);
                    if old.as_ref().is_none_or(|old| {
                        old.1 != editor.document_id || old.2 != editor.buffer.path
                    }) {
                        snapshots.remove(&id);
                    }
                    syntax_pending.remove(&id);
                    if editor.config.syntax
                        && editor.buffer.text.len_bytes() <= editor.config.syntax_max_bytes
                        && language_name(editor.buffer.path.as_deref()) != "Plain text"
                    {
                        let generation = syntax.request_changes(
                            (id << 32) | editor.document_id,
                            editor.buffer.revision,
                            editor.buffer.text.clone(),
                            editor.buffer.path.as_deref(),
                            editor.buffer.recent_changes(),
                        );
                        syntax_pending.insert(
                            id,
                            PendingSyntax {
                                document: editor.document_id,
                                revision: editor.buffer.revision,
                                generation,
                            },
                        );
                    } else {
                        spans.remove(&id);
                        snapshots.remove(&id);
                    }
                }
            }
            snapshots.retain(|id, _| app.workspace.buffers.contains_key(id));
            retire_syntax_pending(&mut syntax_pending, &app);
            syntax.retain_documents(
                &syntax_pending
                    .iter()
                    .map(|(id, stamp)| (id << 32) | stamp.document)
                    .collect(),
            );
            versions.retain(|id, _| app.workspace.buffers.contains_key(id));
        }
        let mut syntax_received = 0;
        for result in syntax.results.try_iter().take(8) {
            syntax_received += 1;
            let id = result.document >> 32;
            let current = complete_syntax_pending(
                &mut syntax_pending,
                id,
                PendingSyntax {
                    document: result.document & 0xffff_ffff,
                    revision: result.revision,
                    generation: result.generation,
                },
            );
            if current
                && let Some(e) = app.workspace.buffers.get(&id)
                && result.document == ((id << 32) | e.document_id)
                && e.buffer.revision == result.revision
                && versions.get(&id).is_some_and(|v| v.2 == e.buffer.path)
                && e.config.syntax
                && e.buffer.text.len_bytes() <= e.config.syntax_max_bytes
            {
                snapshots.insert(id, (result.revision, result.highlights));
                syntax_pending.remove(&id);
                dirty = true;
            }
        }
        if syntax_received == 8 {
            crate::background::wake();
        }
        if dirty {
            spans.clear();
            let size = terminal.size()?;
            let height = usize::from(size.height).max(1);
            let width = usize::from(size.width).max(1);
            let mut visible = Vec::new();
            app.workspace.tabs[app.workspace.active_tab]
                .layout
                .leaves(&mut visible);
            for view in visible {
                let view = &app.workspace.views[&view];
                let editor = &app.workspace.buffers[&view.buffer];
                if let Some((revision, highlights)) = snapshots.get(&view.buffer)
                    && let Some(changes) = editor.buffer.changes_since(*revision)
                {
                    let row = editor
                        .buffer
                        .text
                        .char_to_line(view.cursor.min(editor.buffer.text.len_chars()));
                    for center in [view.top, row] {
                        let first = center
                            .saturating_sub(height * 2)
                            .min(editor.buffer.text.len_lines());
                        let last = (center + height * 2 + 1).min(editor.buffer.text.len_lines());
                        for line in first..last {
                            let text = editor.buffer.line_slice(line);
                            let cursor_column = if line == row {
                                editor.buffer.column_at(
                                    view.cursor.min(editor.buffer.text.len_chars()),
                                    editor.config.tab_width,
                                )
                            } else {
                                view.left
                            };
                            for left in [view.left, cursor_column.saturating_sub(width)] {
                                let from = editor
                                    .buffer
                                    .display_start(
                                        line,
                                        left.saturating_sub(width),
                                        editor.config.tab_width,
                                    )
                                    .0;
                                let to = editor
                                    .buffer
                                    .display_start(
                                        line,
                                        left.saturating_add(width * 3),
                                        editor.config.tab_width,
                                    )
                                    .0;
                                let start = editor.buffer.text.line_to_byte(line);
                                let range =
                                    start + text.char_to_byte(from)..start + text.char_to_byte(to);
                                spans.entry(view.buffer).or_default().extend(
                                    crate::syntax::project_highlights(highlights, &changes, range),
                                );
                            }
                        }
                    }
                }
            }
            for spans in spans.values_mut() {
                spans.sort_unstable_by_key(|h| (h.range.start, h.range.end));
                spans.dedup_by(|a, b| a.range == b.range && a.token == b.token);
            }
        }
        let now = Instant::now();
        let animating = visuals.active(now);
        if dirty || (animating && clock.due(now)) {
            let mode = app.mode();
            if cursor_mode != Some(mode) {
                output(terminal.backend_mut(), Some(mode), None, &[])?;
                cursor_mode = Some(mode);
            }
            let build_timer = crate::perf::Timer::start();
            #[cfg(feature = "perf")]
            let mut built_at = Instant::now();
            if dirty || !visuals.tick_particles(now) {
                terminal.draw(|f| {
                    let construction = crate::perf::Timer::start();
                    if dirty || !visuals.redraw_cursor(f, now, &theme) {
                        workspace_ui::draw(
                            f,
                            &mut app,
                            &theme,
                            &spans,
                            &mut visuals,
                            keymap.pending(),
                            now,
                        )
                    }
                    construction.finish("frame_construction");
                    #[cfg(feature = "perf")]
                    {
                        built_at = Instant::now();
                    }
                })?;
            }
            let (particles, viewport) = visuals.particles();
            let commands = graphics.present(
                particles,
                viewport,
                visuals.effects.pixiedust.lifetime_ms,
                visuals.effects.pixiedust.fps.min(visuals.effects.fps),
                now,
            );
            if !commands.is_empty() {
                output(terminal.backend_mut(), None, None, commands)?;
            }
            graphics.frame_cost(now.elapsed(), visuals.frame_interval());
            build_timer.finish("frame_total");
            #[cfg(feature = "perf")]
            {
                crate::perf::record("frame_diff_output", built_at.elapsed());
                crate::perf::frame_completed();
            }
            let finished = Instant::now();
            let active = visuals.active(finished);
            clock.presented(now, finished, active);
            timer.set_active(active);
            dirty = false;
        }
        for effect in app.effects.drain(..) {
            match effect {
                Effect::Clipboard(text) => {
                    if app.workspace.config.clipboard_osc52 && text.len() <= 100_000 {
                        output(terminal.backend_mut(), None, Some(&text), &[])?;
                    }
                }
            }
        }
        let mut timeout = if visuals.active(Instant::now()) {
            clock.wait(Instant::now()).unwrap_or_default()
        } else if app.pending() {
            Duration::from_millis(16)
        } else {
            Duration::from_secs(3600)
        };
        if let Some(wait) = app.ide.next_wait(&app) {
            timeout = timeout.min(wait);
        }
        if let Some(wait) = graphics.waiting(Instant::now()) {
            timeout = timeout.min(wait);
        }
        // Crossterm's Windows wait truncates fractional milliseconds. Round up
        // instead of spinning through zero-timeout polls near a frame deadline.
        let event = match input_rx.recv_timeout(poll_timeout(timeout)) {
            Ok(Incoming::Input(event)) => event,
            Ok(Incoming::Wake) => {
                wake_pending.store(false, std::sync::atomic::Ordering::Release);
                continue;
            }
            Ok(Incoming::Graphics(reply)) => {
                graphics.reply(&reply, Instant::now());
                dirty = true;
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Ok(app),
        };
        let mut next_event = Some(event);
        let batch_started = Instant::now();
        let mut batch_count = 0;
        while let Some(input) = next_event.take() {
            #[cfg(feature = "perf")]
            crate::perf::dispatched(input.enqueued);
            let action_timer = crate::perf::Timer::start();
            let event = input.event?;
            batch_count += 1;
            let context = (
                app.mode(),
                app.workspace.focused(),
                app.workspace.active_buffer(),
            );
            match event {
                Event::Key(key) if key_events.accept(&key) => {
                    let mut key_dirty = true;
                    let pending_before = keymap.pending().to_owned();
                    if app.ide.terminal_focus {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char(']')
                        {
                            app.ide.terminal_focus = false;
                        } else if key.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                            && !app.ide.terminals.is_empty()
                        {
                            let n = app.ide.terminals.len();
                            app.ide.terminal_active = if key.code == KeyCode::PageUp {
                                (app.ide.terminal_active + n - 1) % n
                            } else {
                                (app.ide.terminal_active + 1) % n
                            };
                        } else if key
                            .modifiers
                            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                            && matches!(key.code, KeyCode::Char('x' | 'X'))
                        {
                            app.command("TerminalClose");
                        } else if let Some(session) = app.ide.terminals.get(app.ide.terminal_active)
                        {
                            if key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                            {
                                session.scroll(if key.code == KeyCode::PageUp { 10 } else { -10 });
                            } else {
                                let application = session.application_cursor();
                                if let Some(bytes) = crate::terminal::key_bytes(key, application)
                                    && !session.input(bytes)
                                {
                                    app.notify("Terminal input queue is full; retry", true);
                                }
                            }
                        }
                    } else if let Some(action) = keymap.resolve(app.mode(), key) {
                        let plain_navigation = app.ide.panel.is_none()
                            && app.ide.prompt.is_none()
                            && app.picker.is_none()
                            && app.dialog.is_none()
                            && !app.explorer.focused
                            && !app.workspace.editor().help
                            && matches!(
                                action,
                                crate::input::Action::Up
                                    | crate::input::Action::Down
                                    | crate::input::Action::Left
                                    | crate::input::Action::Right
                                    | crate::input::Action::Home
                                    | crate::input::Action::End
                                    | crate::input::Action::Top
                                    | crate::input::Action::Bottom
                                    | crate::input::Action::PageUp
                                    | crate::input::Action::PageDown
                                    | crate::input::Action::WordBack
                                    | crate::input::Action::WordForward
                            );
                        let cursor = app.workspace.editor().buffer.cursor;
                        app.act(action);
                        if plain_navigation {
                            key_dirty = cursor != app.workspace.editor().buffer.cursor;
                        }
                        if action == crate::input::Action::Noop {
                            key_dirty = pending_before != keymap.pending();
                        }
                    } else if !app.workspace.editor().help
                        && let Some(c) = text_character(key)
                    {
                        key_dirty =
                            matches!(app.workspace.editor().mode, Mode::Insert | Mode::Command)
                                || app.ide.focused()
                                || app.picker.is_some()
                                || app.dialog.is_some();
                        app.type_text(c.encode_utf8(&mut [0; 4]));
                    }
                    dirty |= key_dirty || pending_before != keymap.pending();
                }
                Event::Paste(text) => {
                    if app.ide.terminal_focus {
                        if let Some(session) = app.ide.terminals.get(app.ide.terminal_active) {
                            let bracketed = session.bracketed_paste();
                            let bytes = if bracketed {
                                format!("\x1b[200~{text}\x1b[201~")
                            } else {
                                text
                            };
                            session.input(bytes.into_bytes());
                        }
                    } else {
                        app.paste(&text);
                    }
                    dirty = true;
                }
                Event::Resize(_, _) => {
                    if graphics.native_candidate() {
                        graphics.resize(
                            CellSize::detect(),
                            visuals.effects,
                            particle_color(&theme, visuals.effects),
                        );
                    }
                    visuals.reset();
                    dirty = true;
                }
                _ => {}
            }
            if context
                != (
                    app.mode(),
                    app.workspace.focused(),
                    app.workspace.active_buffer(),
                )
            {
                keymap.clear();
                visuals.reset();
            }
            action_timer.finish("input_action");
            #[cfg(feature = "perf")]
            crate::perf::completed(input.enqueued);
            if batch_count >= 32 || batch_started.elapsed() >= Duration::from_millis(2) {
                break;
            }
            next_event = match input_rx.try_recv() {
                Ok(Incoming::Input(event)) => Some(event),
                Ok(Incoming::Wake) => {
                    wake_pending.store(false, std::sync::atomic::Ordering::Release);
                    None
                }
                Ok(Incoming::Graphics(reply)) => {
                    graphics.reply(&reply, Instant::now());
                    dirty = true;
                    None
                }
                Err(_) => None,
            };
        }
    }
}

/// A phase-locked clock: input redraws never postpone the next animation frame.
/// Late delivery advances to the next deadline, without burst catch-up renders.
struct FrameClock {
    interval: Duration,
    next: Option<Instant>,
}
impl FrameClock {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            next: None,
        }
    }
    fn due(&self, now: Instant) -> bool {
        self.next.is_some_and(|next| now >= next)
    }
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.next.map(|next| next.saturating_duration_since(now))
    }
    fn presented(&mut self, started: Instant, finished: Instant, active: bool) {
        if !active {
            self.next = None;
            return;
        }
        let next = self.next.get_or_insert(started + self.interval);
        if *next <= finished {
            let remainder =
                finished.saturating_duration_since(*next).as_nanos() % self.interval.as_nanos();
            *next = finished + Duration::from_nanos((self.interval.as_nanos() - remainder) as u64);
        }
    }
}
fn poll_timeout(timeout: Duration) -> Duration {
    Duration::from_millis(
        timeout.as_millis() as u64 + u64::from(!timeout.subsec_nanos().is_multiple_of(1_000_000)),
    )
}
/// Request precise Windows waits only while animating; idle has no timer lease.
#[derive(Default)]
struct AnimationTimer {
    #[cfg(windows)]
    active: bool,
}
impl AnimationTimer {
    fn set_active(&mut self, active: bool) {
        #[cfg(windows)]
        if active != self.active {
            if active {
                // SAFETY: no pointers; pair every successful request in Drop.
                self.active = unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) == 0 };
            } else {
                // SAFETY: matches this guard's successful timeBeginPeriod(1).
                unsafe {
                    windows_sys::Win32::Media::timeEndPeriod(1);
                }
                self.active = false;
            }
        }
        #[cfg(not(windows))]
        let _ = active;
    }
}
impl Drop for AnimationTimer {
    fn drop(&mut self) {
        self.set_active(false);
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;
    #[test]
    fn keyboard_redraws_do_not_postpone_animation_deadlines() {
        let start = Instant::now();
        let interval = Duration::from_nanos(16_666_667);
        let mut clock = FrameClock::new(interval);
        clock.presented(start, start + Duration::from_millis(2), true);
        for ms in [5, 9, 12] {
            clock.presented(
                start + Duration::from_millis(ms),
                start + Duration::from_millis(ms + 1),
                true,
            );
            assert_eq!(clock.next, Some(start + interval));
        }
        assert!(clock.due(start + interval));
        for frame in 1..=120 {
            let deadline = start + interval * frame;
            clock.presented(
                deadline + Duration::from_micros(100),
                deadline + Duration::from_millis(2),
                true,
            );
            assert_eq!(clock.next, Some(start + interval * (frame + 1)));
        }
    }
    #[test]
    fn slow_frames_have_no_catch_up_burst_and_idle_has_no_deadline() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let mut clock = FrameClock::new(interval);
        clock.presented(start, start + Duration::from_millis(63), true);
        assert_eq!(clock.next, Some(start + Duration::from_millis(70)));
        assert_eq!(
            clock.wait(start + Duration::from_millis(65)),
            Some(Duration::from_millis(5))
        );
        clock.presented(
            start + Duration::from_millis(70),
            start + Duration::from_millis(71),
            false,
        );
        assert!(!clock.due(start + Duration::from_secs(10)));
        assert_eq!(clock.wait(start), None);
    }
    #[test]
    fn fractional_poll_waits_round_up_instead_of_busy_spinning() {
        assert_eq!(poll_timeout(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            poll_timeout(Duration::from_nanos(1)),
            Duration::from_millis(1)
        );
        assert_eq!(
            poll_timeout(Duration::from_nanos(16_666_667)),
            Duration::from_millis(17)
        );
        assert_eq!(
            poll_timeout(Duration::from_millis(16)),
            Duration::from_millis(16)
        );
    }
    #[test]
    fn output_buffer_batches_command_fragments_and_flushes_each_frame() {
        #[derive(Default)]
        struct Sink {
            writes: usize,
            bytes: usize,
        }
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.writes += 1;
                self.bytes += b.len();
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = std::io::BufWriter::with_capacity(64 * 1024, Sink::default());
        for frame in 1..=60 {
            for _ in 0..1000 {
                output.write_all(b"\x1b[38;2;196;167;231m").unwrap();
            }
            output.flush().unwrap();
            assert_eq!(output.get_ref().writes, frame);
        }
        assert!(output.get_ref().bytes > 0);
    }
}

#[cfg(test)]
mod responsiveness_tests {
    use super::*;
    use crate::{
        buffer::Buffer, config::Config, editor::Editor, storage::History, workspace::Workspace,
    };
    use ratatui::backend::TestBackend;
    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.lsp.enabled = false;
        config.git.enabled = false;
        config.syntax = false;
        config.preset = crate::config::Preset::Minimal;
        let mut editor = Editor::new(config);
        editor.set_buffer(Buffer::default());
        let mut app = App::with_history_path(
            Workspace::new(editor, dir.path().into()),
            History::default(),
            dir.path().join("history"),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.pending() && Instant::now() < deadline {
            app.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.pending());
        (dir, app)
    }
    fn key(code: KeyCode) -> Incoming {
        Incoming::input(Ok(Event::Key(crossterm::event::KeyEvent::from(code))))
    }
    #[test]
    fn pixel_only_frames_do_not_construct_or_diff_ratatui_scenes() {
        let (_dir, mut app) = app();
        app.workspace.config.preset = crate::config::Preset::Balanced;
        for editor in app.workspace.buffers.values_mut() {
            editor.config = app.workspace.config.clone();
            editor.set_buffer(Buffer::new(ropey::Rope::from_str(&"x".repeat(80)), None));
        }
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            for _ in 0..20 {
                tx.send(key(KeyCode::Char('d'))).unwrap();
            }
            std::thread::sleep(Duration::from_millis(650));
        });
        let mut graphics = crate::pixiedust::graphics::tests::ready().0;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut updates = 0;
        let result = run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            Default::default(),
            (
                |_, _, _, bytes: &[u8]| {
                    if !bytes.is_empty() {
                        updates += 1;
                    }
                    Ok(())
                },
                &mut graphics,
            ),
        )
        .unwrap();
        producer.join().unwrap();
        assert_eq!(result.workspace.editor().buffer.cursor, 20);
        assert!(updates > 5, "particle-only updates must run");
        assert!(
            terminal.get_frame().count() <= 4,
            "unchanged text must not redraw for particle expiration"
        );
        assert!(graphics.waiting(Instant::now()).is_none());
    }
    #[cfg(feature = "perf")]
    #[test]
    #[ignore = "release mixed navigation/typing replay comparing off, smear and negotiated pixiedust"]
    fn pixiedust_input_latency() {
        for effect in [
            crate::config::CursorEffect::Off,
            crate::config::CursorEffect::Smear,
            crate::config::CursorEffect::Pixiedust,
        ] {
            crate::perf::reset();
            let mut total_bytes = 0;
            let mut total_frames = 0;
            for _ in 0..3 {
                let (_dir, mut app) = app();
                app.workspace.config.preset = crate::config::Preset::Balanced;
                app.workspace.config.visual.cursor_effect = effect;
                for editor in app.workspace.buffers.values_mut() {
                    editor.config = app.workspace.config.clone();
                    editor.set_buffer(Buffer::new(ropey::Rope::from_str(&"x".repeat(100)), None));
                }
                let map = Keymap::new(&app.workspace.config).unwrap();
                let (tx, rx) = std::sync::mpsc::channel();
                let producer = std::thread::spawn(move || {
                    let start = Instant::now();
                    for i in 0..202 {
                        let due = start + Duration::from_secs_f64(i as f64 / 120.0);
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                        let code = if i < 160 {
                            KeyCode::Char(if i % 64 < 32 { 'd' } else { 'a' })
                        } else if i == 160 {
                            KeyCode::Char('i')
                        } else if i == 201 {
                            KeyCode::Esc
                        } else {
                            KeyCode::Char('z')
                        };
                        tx.send(key(code)).unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(700));
                    let before = crate::perf::samples("background_poll").len();
                    std::thread::sleep(Duration::from_millis(150));
                    let after = crate::perf::samples("background_poll").len();
                    assert_eq!(before, after, "settled effect must not poll");
                });
                let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
                let mut graphics = if effect == crate::config::CursorEffect::Pixiedust {
                    crate::pixiedust::graphics::tests::ready().0
                } else {
                    Graphics::disabled()
                };
                let app = run_loop(
                    &mut terminal,
                    app,
                    map,
                    Theme::default(),
                    rx,
                    Default::default(),
                    (
                        |_, _, _, bytes: &[u8]| {
                            total_bytes += bytes.len();
                            Ok(())
                        },
                        &mut graphics,
                    ),
                )
                .unwrap();
                graphics.finish();
                producer.join().unwrap();
                total_frames += terminal.get_frame().count();
                assert_eq!(app.workspace.editor().buffer.text.len_chars(), 140);
                assert_eq!(app.workspace.editor().mode, Mode::Normal);
            }
            println!(
                "pixiedust_input_latency/{effect:?}: graphics_bytes={total_bytes} frames={total_frames}\n{}",
                crate::perf::report()
            );
        }
    }
    #[test]
    fn old_syntax_generation_cannot_retire_a_rename_or_configuration_request() {
        let old = PendingSyntax {
            document: 1,
            revision: 0,
            generation: 1,
        };
        let new = PendingSyntax {
            generation: 2,
            ..old
        };
        let mut pending = BTreeMap::from([(1, new)]);
        assert!(!complete_syntax_pending(&mut pending, 1, old));
        assert_eq!(pending.get(&1), Some(&new));
        assert!(complete_syntax_pending(&mut pending, 1, new));
        assert!(pending.is_empty());
    }
    #[test]
    fn hidden_edited_replaced_and_closed_syntax_generations_retire() {
        let (_dir, mut app) = app();
        let id = app.workspace.active_buffer();
        let document = app.workspace.editor().document_id;
        let mut pending = BTreeMap::from([(
            id,
            PendingSyntax {
                document,
                revision: 0,
                generation: 1,
            },
        )]);
        // A parse of revision zero is outstanding, then one input batch edits A
        // and hides it behind a new buffer before syntax scheduling runs again.
        app.workspace.editor_mut().buffer.insert("é\n");
        app.workspace.add_buffer(Buffer::default());
        retire_syntax_pending(&mut pending, &app);
        assert!(pending.is_empty());
        let editor = app.workspace.buffers.get_mut(&id).unwrap();
        pending.insert(
            id,
            PendingSyntax {
                document,
                revision: editor.buffer.revision,
                generation: 2,
            },
        );
        editor.set_buffer(Buffer::default());
        retire_syntax_pending(&mut pending, &app);
        assert!(pending.is_empty());
        pending.insert(
            id,
            PendingSyntax {
                document: app.workspace.buffers[&id].document_id,
                revision: 0,
                generation: 3,
            },
        );
        app.workspace.close_buffer(id, true).unwrap();
        retire_syntax_pending(&mut pending, &app);
        assert!(pending.is_empty());
        assert!(!app.pending());
    }

    #[cfg(feature = "perf")]
    #[test]
    #[ignore = "release idle wake regression; run serially with perf recorder"]
    fn stale_hidden_syntax_does_not_periodically_poll() {
        let (_dir, mut app) = app();
        app.workspace.editor_mut().config.syntax = true;
        app.workspace.editor_mut().set_buffer(Buffer::new(
            ropey::Rope::from_str(&"fn x() {}\n".repeat(100_000)),
            Some("pending.rs".into()),
        ));
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Arc::new(tx);
        for code in [
            KeyCode::Char('i'),
            KeyCode::Char('z'),
            KeyCode::Esc,
            KeyCode::Char(':'),
            KeyCode::Char('n'),
            KeyCode::Char('e'),
            KeyCode::Char('w'),
            KeyCode::Enter,
        ] {
            tx.send(key(code)).unwrap();
        }
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wake_flag = flag.clone();
        let weak_tx = std::sync::Arc::downgrade(&tx);
        let guard = crate::background::WakeGuard::install(std::sync::Arc::new(move || {
            if !wake_flag.swap(true, std::sync::atomic::Ordering::AcqRel)
                && let Some(tx) = weak_tx.upgrade()
            {
                let _ = tx.send(Incoming::Wake);
            }
        }));
        let producer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            drop(tx);
        });
        crate::perf::reset();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let app = run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            flag,
            (|_, _, _, _| Ok(()), &mut Graphics::disabled()),
        )
        .unwrap();
        drop(guard);
        producer.join().unwrap();
        assert_eq!(app.workspace.editor().buffer.text.len_chars(), 0);
        let polls = crate::perf::samples("background_poll").len();
        assert!(
            polls < 10,
            "settled syntax must not poll every 16 ms: {polls}"
        );
        println!("250 ms stale/hidden syntax lifecycle: background_polls={polls}");
    }

    #[cfg(feature = "perf")]
    #[test]
    #[ignore = "release individual-sample mixed-load benchmark"]
    fn mixed_load_individual_latency() {
        let (dir, mut app) = app();
        let executable = dir
            .path()
            .join(if cfg!(windows) { "mock.exe" } else { "mock" });
        let mut command = std::process::Command::new("rustc");
        command
            .args(["--edition=2024", "tests/fixtures/mock_lsp.rs", "-o"])
            .arg(&executable);
        crate::setup::hide_window(&mut command);
        assert!(command.output().unwrap().status.success());
        app.workspace.config.lsp.enabled = true;
        app.workspace.config.lsp.servers = [("alpha", "rust"), ("beta", "python")]
            .into_iter()
            .map(|(name, lang)| crate::lsp::ServerConfig {
                name: name.into(),
                command: executable.to_string_lossy().into(),
                args: vec![
                    dir.path().join(name).to_string_lossy().into(),
                    "--flood".into(),
                    "--stream".into(),
                    "--incremental".into(),
                ],
                languages: vec![lang.into()],
                ..Default::default()
            })
            .collect();
        let text = "fn main() { let value = 42; }\n".repeat(2000);
        let rust_path = dir.path().join("main.rs");
        std::fs::write(&rust_path, &text).unwrap();
        app.workspace
            .editor_mut()
            .set_buffer(Buffer::new(ropey::Rope::from_str(&text), Some(rust_path)));
        app.workspace.editor_mut().config.syntax = true;
        let python_path = dir.path().join("other.py");
        std::fs::write(&python_path, "value = 1\n").unwrap();
        app.workspace.insert_buffer(Buffer::new(
            ropey::Rope::from_str("value = 1\n"),
            Some(python_path),
        ));
        app.ide.terminals.push(crate::terminal::Session::start(
            crate::terminal::Config {
                shell: executable.to_string_lossy().into(),
                args: vec!["--terminal-output".into()],
                ..Default::default()
            },
            dir.path().into(),
            10,
            100,
        ));
        app.ide.terminal_visible = true;
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let wake_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wake = wake_pending.clone();
        let wake_tx = tx.clone();
        let guard = crate::background::WakeGuard::install(std::sync::Arc::new(move || {
            if !wake.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let _ = wake_tx.send(Incoming::Wake);
            }
        }));
        crate::perf::reset();
        let producer = std::thread::spawn(move || {
            // Let servers start, then overlap typing with preparation/publication.
            std::thread::sleep(Duration::from_millis(100));
            tx.send(key(KeyCode::Char('i'))).unwrap();
            let start = Instant::now();
            for i in 0..240 {
                let due = start + Duration::from_secs_f64(i as f64 / 120.0);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                tx.send(key(KeyCode::Char('z'))).unwrap();
            }
            tx.send(key(KeyCode::Esc)).unwrap();
            // Quit through the actual keymap; workers are shut down by runtime.
            for code in [
                KeyCode::Char(':'),
                KeyCode::Char('q'),
                KeyCode::Char('!'),
                KeyCode::Enter,
            ] {
                tx.send(key(code)).unwrap();
            }
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let app = run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            wake_pending,
            (|_, _, _, _| Ok(()), &mut Graphics::disabled()),
        )
        .unwrap();
        drop(guard);
        producer.join().unwrap();
        assert!(
            app.workspace
                .editor()
                .buffer
                .text
                .to_string()
                .contains(&"z".repeat(240))
        );
        assert!(crate::perf::samples("input_to_action").len() >= 240);
        assert!(
            crate::perf::traces()
                .iter()
                .any(|(stage, _, _)| *stage == "lsp_prepared")
        );
        assert!(
            crate::perf::traces()
                .iter()
                .any(|(stage, _, _)| *stage == "edit_applied")
        );
        println!("{}", crate::perf::report());
        for (server, input, events) in app.ide.transport_queues() {
            println!("{server}: transport={input:?} ui_events={events:?}");
        }
    }
    #[test]
    fn queued_input_preserves_modes_and_batches_without_losing_text() {
        let (_dir, app) = app();
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(key(KeyCode::Char('i'))).unwrap();
        for _ in 0..1000 {
            tx.send(key(KeyCode::Char('z'))).unwrap();
        }
        tx.send(key(KeyCode::Esc)).unwrap();
        tx.send(key(KeyCode::Char('a'))).unwrap(); // Normal-mode WASD left
        drop(tx);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let at = Instant::now();
        let app = run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            Default::default(),
            (|_, _, _, _| Ok(()), &mut Graphics::disabled()),
        )
        .unwrap();
        let frames = terminal.get_frame().count();
        assert_eq!(app.workspace.editor().buffer.text, "z".repeat(1000));
        assert_eq!(app.workspace.editor().mode, Mode::Normal);
        assert_eq!(app.workspace.editor().buffer.cursor, 999);
        assert!(
            frames < 1003,
            "queued keys should share frames, got {frames}"
        );
        println!(
            "runtime 1003 ordered events: frames={frames} elapsed_us={}",
            at.elapsed().as_micros()
        );
    }
    #[test]
    fn idle_wakes_and_boundary_navigation_do_not_redraw() {
        let (_dir, app) = app();
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..100 {
            tx.send(Incoming::Wake).unwrap();
            tx.send(key(KeyCode::Char('a'))).unwrap();
        }
        drop(tx);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            Default::default(),
            (|_, _, _, _| Ok(()), &mut Graphics::disabled()),
        )
        .unwrap();
        assert_eq!(
            terminal.get_frame().count(),
            1,
            "idle wakes/no-op navigation must not draw"
        );
    }
    #[test]
    #[ignore = "release sustained-input benchmark"]
    fn sustained_input_has_no_growing_backlog() {
        let (_dir, app) = app();
        let map = Keymap::new(&app.workspace.config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            tx.send(key(KeyCode::Char('i'))).unwrap();
            let start = Instant::now();
            for i in 0..240 {
                let due = start + Duration::from_secs_f64(i as f64 / 120.0);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                tx.send(key(KeyCode::Char('z'))).unwrap();
            }
            done_tx.send(Instant::now()).unwrap();
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let app = run_loop(
            &mut terminal,
            app,
            map,
            Theme::default(),
            rx,
            Default::default(),
            (|_, _, _, _| Ok(()), &mut Graphics::disabled()),
        )
        .unwrap();
        let finished = Instant::now();
        producer.join().unwrap();
        let tail = finished.duration_since(done_rx.recv().unwrap());
        assert_eq!(app.workspace.editor().buffer.text, "z".repeat(240));
        assert!(
            tail < Duration::from_millis(100),
            "input backlog tail={tail:?}"
        );
        println!(
            "120 events/s for 240 events: final drain_us={} frames={}",
            tail.as_micros(),
            terminal.get_frame().count()
        );
    }
}
