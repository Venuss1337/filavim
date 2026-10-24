//! PTY creation, writes, resize and cleanup run on a session worker.
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    num::NonZeroU16,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub shell: String,
    pub args: Vec<String>,
    pub scrollback: usize,
    pub height: u16,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            shell: std::env::var(if cfg!(windows) { "COMSPEC" } else { "SHELL" })
                .unwrap_or_else(|_| if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into()),
            args: Vec::new(),
            scrollback: 2000,
            height: 12,
        }
    }
}
enum Input {
    Bytes(Vec<u8>),
    Output(Vec<u8>),
    Resize(u16, u16),
    Scroll(isize),
    Eof,
    Close,
    Wake,
}
#[derive(Default)]
struct Responses {
    bytes: Vec<u8>,
}
impl vt100::Callbacks for Responses {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        _i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let parameter = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        if c == 'n' && parameter == 6 {
            let (y, x) = screen.cursor_position();
            self.bytes
                .extend_from_slice(format!("\x1b[{};{}R", y + 1, x + 1).as_bytes());
        } else if c == 'n' && parameter == 5 {
            self.bytes.extend_from_slice(b"\x1b[0n");
        } else if c == 'c' {
            self.bytes.extend_from_slice(b"\x1b[?1;2c");
        }
    }
}
/// Immutable visible cells and input modes; scrollback stays on the PTY worker.
#[derive(Clone)]
pub struct ViewportSnapshot {
    cells: Vec<vt100::Cell>,
    rows: NonZeroU16,
    cols: NonZeroU16,
    cursor: (u16, u16),
    scrollback: usize,
    hidden: bool,
    application: bool,
    bracketed: bool,
}
impl ViewportSnapshot {
    pub fn capture(screen: &vt100::Screen) -> Self {
        let (rows, cols) = screen.size();
        let mut cells = Vec::with_capacity(rows.get() as usize * cols.get() as usize);
        for y in 0..rows.get() {
            for x in 0..cols.get() {
                cells.push(screen.cell(y, x).unwrap().clone());
            }
        }
        Self {
            cells,
            rows,
            cols,
            cursor: screen.cursor_position(),
            scrollback: screen.scrollback(),
            hidden: screen.hide_cursor(),
            application: screen.application_cursor(),
            bracketed: screen.bracketed_paste(),
        }
    }
    pub fn cell(&self, y: u16, x: u16) -> Option<&vt100::Cell> {
        if y >= self.rows.get() || x >= self.cols.get() {
            return None;
        }
        self.cells
            .get(y as usize * self.cols.get() as usize + x as usize)
    }
    pub fn size(&self) -> (NonZeroU16, NonZeroU16) {
        (self.rows, self.cols)
    }
    pub fn cursor_position(&self) -> (u16, u16) {
        self.cursor
    }
    pub fn scrollback(&self) -> usize {
        self.scrollback
    }
    pub fn hide_cursor(&self) -> bool {
        self.hidden
    }
    pub fn application_cursor(&self) -> bool {
        self.application
    }
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed
    }
    pub fn contents(&self) -> String {
        (0..self.rows.get())
            .map(|y| {
                (0..self.cols.get())
                    .map(|x| self.cell(y, x).unwrap().contents())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
pub struct Session {
    tx: mpsc::SyncSender<Input>,
    priority: mpsc::SyncSender<Input>,
    pub screen: Arc<Mutex<Arc<ViewportSnapshot>>>,
    pub changed: Arc<AtomicBool>,
    pub ended: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub size: (u16, u16),
    pub display: Arc<ViewportSnapshot>,
    visible: Arc<AtomicBool>,
    modes: Arc<AtomicU8>,
    pub publications: Arc<AtomicU64>,
}
impl Session {
    pub fn start(config: Config, root: PathBuf, rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let parser = vt100::Parser::new_with_callbacks(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            config.scrollback,
            Responses::default(),
        );
        let display = Arc::new(ViewportSnapshot::capture(parser.screen()));
        let screen = Arc::new(Mutex::new(display.clone()));
        let changed = Arc::new(AtomicBool::new(true));
        let ended = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let visible = Arc::new(AtomicBool::new(true));
        let modes = Arc::new(AtomicU8::new(0));
        let publications = Arc::new(AtomicU64::new(0));
        let publication = Publication {
            screen: screen.clone(),
            dirty: changed.clone(),
            visible: visible.clone(),
            modes: modes.clone(),
            count: publications.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(128);
        let (priority, priority_rx) = mpsc::sync_channel(128);
        let worker_tx = tx.clone();
        let dirty = changed.clone();
        let done = ended.clone();
        let failure = error.clone();
        thread::spawn(move || {
            if let Err(e) = session(
                config,
                root,
                parser,
                (rx, priority_rx),
                worker_tx,
                publication,
            ) {
                *failure.lock().unwrap() = Some(format!("{e:#}"));
            }
            done.store(true, Ordering::Release);
            dirty.store(true, Ordering::Release);
            crate::background::wake();
        });
        Self {
            tx,
            priority,
            screen,
            changed,
            ended,
            error,
            size: (rows, cols),
            display,
            visible,
            modes,
            publications,
        }
    }
    pub fn set_visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::AcqRel) != visible {
            let _ = self.tx.try_send(Input::Wake);
        }
    }
    pub fn application_cursor(&self) -> bool {
        self.modes.load(Ordering::Acquire) & 1 != 0
    }
    pub fn bracketed_paste(&self) -> bool {
        self.modes.load(Ordering::Acquire) & 2 != 0
    }
    fn control(&self, input: Input) -> bool {
        let sent = self.priority.try_send(input).is_ok();
        if sent {
            let _ = self.tx.try_send(Input::Wake);
        }
        sent
    }
    pub fn input(&self, bytes: Vec<u8>) -> bool {
        self.control(Input::Bytes(bytes))
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = (rows.max(1), cols.max(1));
        if size != self.size && self.control(Input::Resize(size.0, size.1)) {
            self.size = size;
        }
    }
    pub fn scroll(&self, amount: isize) {
        self.control(Input::Scroll(amount));
    }
    pub fn poll(&mut self) -> bool {
        if !self.changed.swap(false, Ordering::AcqRel) {
            return false;
        }
        if let Ok(screen) = self.screen.try_lock() {
            self.display = screen.clone();
            true
        } else {
            self.changed.store(true, Ordering::Release);
            crate::background::wake();
            false
        }
    }
    pub fn close(&self) {
        self.control(Input::Close);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let tx = self.priority.clone();
        let wake = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Input::Close);
            let _ = wake.try_send(Input::Wake);
        });
    }
}
struct Child(Box<dyn portable_pty::Child + Send + Sync>);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Publication {
    screen: Arc<Mutex<Arc<ViewportSnapshot>>>,
    dirty: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    modes: Arc<AtomicU8>,
    count: Arc<AtomicU64>,
}
impl Publication {
    fn update_modes(&self, screen: &vt100::Screen) {
        self.modes.store(
            u8::from(screen.application_cursor()) | (u8::from(screen.bracketed_paste()) << 1),
            Ordering::Release,
        );
    }
}
fn session(
    config: Config,
    root: PathBuf,
    mut parser: vt100::Parser<Responses>,
    receivers: (mpsc::Receiver<Input>, mpsc::Receiver<Input>),
    tx: mpsc::SyncSender<Input>,
    publication: Publication,
) -> Result<()> {
    let (rx, priority) = receivers;
    let screen = &publication.screen;
    let dirty = &publication.dirty;
    let (rows, cols) = parser.screen().size();
    let pair = native_pty_system().openpty(PtySize {
        rows: rows.get(),
        cols: cols.get(),
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(config.shell);
    command.args(&config.args);
    command.cwd(root);
    command.env("TERM", "xterm-256color");
    let mut child = Child(pair.slave.spawn_command(command)?);
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Input::Eof);
                    break;
                }
                Ok(n) => {
                    if tx.send(Input::Output(bytes[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut publish = true;
    let mut was_visible = false;
    let mut last_publish = Instant::now() - Duration::from_secs(1);
    let mut last_output = Instant::now();
    let mut exited_at = None;
    loop {
        let visible = publication.visible.load(Ordering::Acquire);
        if visible && !was_visible {
            publish = true;
        }
        was_visible = visible;
        let wait = if publish && visible {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(250)
        };
        match priority
            .try_recv()
            .map_err(|_| ())
            .or_else(|_| rx.recv_timeout(wait))
        {
            Ok(Input::Bytes(bytes)) => {
                parser.screen_mut().set_scrollback(0);
                writer.write_all(&bytes)?;
                writer.flush()?;
                publish = true;
            }
            Ok(Input::Output(bytes)) => {
                last_output = Instant::now();
                parser.process(&bytes);
                let replies = std::mem::take(&mut parser.callbacks_mut().bytes);
                if !replies.is_empty() {
                    writer.write_all(&replies)?;
                    writer.flush()?;
                }
                publish = true;
            }
            Ok(Input::Resize(rows, cols)) => {
                pair.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
                parser.set_size(
                    NonZeroU16::new(rows).context("rows")?,
                    NonZeroU16::new(cols).context("cols")?,
                );
                publish = true;
            }
            Ok(Input::Scroll(amount)) => {
                let scroll = parser.screen().scrollback().saturating_add_signed(amount);
                parser.screen_mut().set_scrollback(scroll);
                publish = true;
            }
            Ok(Input::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Ok(Input::Eof) => {
                *screen.lock().unwrap() = Arc::new(ViewportSnapshot::capture(parser.screen()));
                dirty.store(true, Ordering::Release);
                crate::background::wake();
                return Ok(());
            }
            Ok(Input::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        publication.update_modes(parser.screen());
        if publish
            && visible
            && last_publish.elapsed() >= Duration::from_millis(16)
            && let Ok(mut target) = screen.try_lock()
        {
            *target = Arc::new(ViewportSnapshot::capture(parser.screen()));
            publication.count.fetch_add(1, Ordering::Relaxed);
            publish = false;
            last_publish = Instant::now();
            dirty.store(true, Ordering::Release);
            crate::background::wake();
        }
        if child.0.try_wait()?.is_some() {
            let exited = exited_at.get_or_insert_with(Instant::now);
            // ConPTY may retain its output pipe until the master is closed.
            // Drain queued output, then close the PTY instead of waiting for
            // an EOF that depends on our own master handle being dropped.
            if exited.elapsed() >= Duration::from_millis(100)
                && last_output.elapsed() >= Duration::from_millis(100)
            {
                *screen.lock().unwrap() = Arc::new(ViewportSnapshot::capture(parser.screen()));
                dirty.store(true, Ordering::Release);
                crate::background::wake();
                return Ok(());
            }
        }
    }
}
pub fn key_bytes(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT) && !control;
    let prefix = if application_cursor { "\x1bO" } else { "\x1b[" };
    let s = match key.code {
        KeyCode::Char(c) if control && c.is_ascii() => {
            String::from(char::from((c.to_ascii_uppercase() as u8) & 0x1f))
        }
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Up => format!("{prefix}A"),
        KeyCode::Down => format!("{prefix}B"),
        KeyCode::Right => format!("{prefix}C"),
        KeyCode::Left => format!("{prefix}D"),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::Insert => "\x1b[2~".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        KeyCode::F(n) => match n {
            1 => "\x1bOP",
            2 => "\x1bOQ",
            3 => "\x1bOR",
            4 => "\x1bOS",
            5 => "\x1b[15~",
            6 => "\x1b[17~",
            7 => "\x1b[18~",
            8 => "\x1b[19~",
            9 => "\x1b[20~",
            10 => "\x1b[21~",
            11 => "\x1b[23~",
            12 => "\x1b[24~",
            _ => return None,
        }
        .into(),
        _ => return None,
    };
    Some(if alt { format!("\x1b{s}") } else { s }.into_bytes())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[test]
    fn hidden_modes_are_published_without_capturing_cells() {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(12).unwrap(),
            NonZeroU16::new(80).unwrap(),
            10,
        );
        let publication = Publication {
            screen: Arc::new(Mutex::new(Arc::new(ViewportSnapshot::capture(
                parser.screen(),
            )))),
            dirty: Arc::new(AtomicBool::new(false)),
            visible: Arc::new(AtomicBool::new(false)),
            modes: Arc::new(AtomicU8::new(0)),
            count: Arc::new(AtomicU64::new(0)),
        };
        parser.process(b"\x1b[?1h\x1b[?2004houtput");
        publication.update_modes(parser.screen());
        assert_eq!(publication.modes.load(Ordering::Acquire), 3);
        assert_eq!(publication.count.load(Ordering::Acquire), 0);
        parser.process(b"\x1b[?1l\x1b[?2004l");
        publication.update_modes(parser.screen());
        assert_eq!(publication.modes.load(Ordering::Acquire), 0);
    }
    #[test]
    fn viewport_preserves_cells_modes_and_scrollback_position() {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(4).unwrap(),
            NonZeroU16::new(12).unwrap(),
            2000,
        );
        parser.process(b"\x1b[?1h\x1b[?2004h\x1b[?25l");
        for _ in 0..100 {
            parser.process("\x1b[31m日本 x\r\n".as_bytes());
        }
        parser.screen_mut().set_scrollback(3);
        let view = ViewportSnapshot::capture(parser.screen());
        assert_eq!(view.cells.len(), 48);
        assert_eq!(view.scrollback(), 3);
        assert!(view.application_cursor() && view.bracketed_paste() && view.hide_cursor());
        for y in 0..4 {
            for x in 0..12 {
                assert_eq!(view.cell(y, x), parser.screen().cell(y, x));
            }
        }
    }
    #[test]
    fn hidden_terminal_publications_do_not_dirty_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.git.enabled = false;
        config.lsp.enabled = false;
        let editor = crate::editor::Editor::new(config);
        let mut app = crate::app::App::with_history_path(
            crate::workspace::Workspace::new(editor, dir.path().into()),
            crate::storage::History::default(),
            dir.path().join("history"),
        );
        let (tx, _rx) = mpsc::sync_channel(128);
        let (priority, _priority_rx) = mpsc::sync_channel(128);
        let parser =
            vt100::Parser::new(NonZeroU16::new(4).unwrap(), NonZeroU16::new(12).unwrap(), 0);
        let view = Arc::new(ViewportSnapshot::capture(parser.screen()));
        app.ide.terminals.push(Session {
            visible: Arc::new(AtomicBool::new(false)),
            modes: Arc::new(AtomicU8::new(0)),
            publications: Arc::new(AtomicU64::new(0)),
            tx,
            priority,
            screen: Arc::new(Mutex::new(view.clone())),
            display: view,
            changed: Arc::new(AtomicBool::new(true)),
            ended: Default::default(),
            error: Default::default(),
            size: (4, 12),
        });
        let mut ide = std::mem::take(&mut app.ide);
        assert!(!ide.tick(&mut app));
        ide.terminal_visible = true;
        ide.terminals[0].changed.store(true, Ordering::Release);
        assert!(ide.tick(&mut app));
        // Dropping test-only senders must not leave a cleanup thread waiting.
        ide.terminals.clear();
    }
}
