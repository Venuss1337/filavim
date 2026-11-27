//! GPUI integration point for input, documents, asynchronous services and presentation.
//!
//! `changed` reconciles logical state with services; `request` dispatches effects;
//! `prepare` caches visible shaped rows and `paint` draws the editor. Worker wakes
//! are coalesced through a bounded async channel. Cursor animation is a sibling
//! entity, not another editing authority or a reason to reshape document text.
#[cfg(all(test, feature = "gpui-tests"))]
#[path = "integration_tests.rs"]
mod integration_tests;
#[path = "manager_view.rs"]
mod manager_ui;
#[path = "search_refresh.rs"]
mod search_refresh;
#[path = "shell_view.rs"]
mod shell_view;
use super::{
    FONT_FAMILY,
    design::{self, Tone, space},
    icons, input, io,
    model::Model,
    theme::Theme,
    viewport::VisibleLine,
};
use crate::{
    editor::{Mode, Request},
    input::Action,
    key::KeyCode,
};
use gpui::{prelude::*, *};
use manager_ui::ManagerUi;
use std::{ops::Range, path::PathBuf};

enum Decision {
    Workspace {
        token: u64,
        expanded: bool,
        position: usize,
    },
    Recovery(Vec<crate::recovery::Entry>),
    Close,
    Conflict {
        path: PathBuf,
        actual: crate::files::DiskState,
        overwrite: bool,
    },
}
struct Confirmation {
    decision: Decision,
    document: u64,
    revision: u64,
    selected: usize,
}
/// Indent levels for the visible rows (see `EditorView::indent_guides`).
struct Guides {
    unit: usize,
    rows: Vec<(usize, Option<usize>)>,
}
impl<'a> IntoIterator for &'a Guides {
    type Item = &'a (usize, Option<usize>);
    type IntoIter = std::slice::Iter<'a, (usize, Option<usize>)>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}
const GUTTER: f32 = 68.0;
const PADDING: f32 = 12.0;
#[derive(Clone)]
struct Row {
    source: VisibleLine,
    shape: ShapedLine,
    number: ShapedLine,
    origin: Point<Pixels>,
    search: Vec<(usize, usize)>,
}
struct Cache {
    document: u64,
    revision: u64,
    epoch: u64,
    top: usize,
    left: usize,
    columns: usize,
    rows: usize,
    lines: Vec<Row>,
    search: String,
}
/// The cursor row's number, tinted with the mode color. Shaped once per row/mode
/// change instead of shaping a second number for every visible row.
struct ActiveNumber {
    row: usize,
    color: Hsla,
    font_size: f32,
    line: ShapedLine,
}
struct CommandLine {
    line: ShapedLine,
    font_size: f32,
    color: Hsla,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
}
/// UI-thread owner of documents, focus, popups, viewport cache and service adapters.
/// Keep blocking work behind executor jobs or the existing service workers.
pub struct EditorView {
    pub model: super::documents::Documents,
    manager_ui: ManagerUi,
    pub(super) cursor: Entity<super::cursor_view::CursorView>,
    pub(super) toasts: Entity<super::toast_view::ToastView>,
    focus: FocusHandle,
    theme: Theme,
    pub(super) stage: Entity<super::stage::Stage>,
    /// Presentation motion owned by the shell (tab lens, command lens). Frames
    /// are requested only while one of these is moving.
    clock: super::motion::Clock,
    tab_lens: (super::motion::Motion, super::motion::Motion),
    tab_lens_ready: bool,
    command_presence: super::motion::Motion,
    /// Where the command lens sits (or would sit), in window coordinates.
    lens_frame: Option<Bounds<Pixels>>,
    /// Last explorer width and mode color sent to the stage. The stage is a
    /// cached sibling, so updates are deferred out of this view's draw (a
    /// notification raised mid-frame would be dropped) and sent only on change.
    stage_layout: Option<(Option<f32>, bool)>,
    stage_mode: Option<Hsla>,
    bounds: Option<Bounds<Pixels>>,
    cell: Pixels,
    gutter: f32,
    cache: Option<Cache>,
    active_number: Option<ActiveNumber>,
    command_line: Option<CommandLine>,
    follow_cursor: bool,
    busy: bool,
    prompt: bool,
    dragging: bool,
    scroll_y: f32,
    scroll_x: f32,
    pub painted_frames: u64,
    shaped_rows: u64,
    reused_rows: u64,
    confirmation: Option<Confirmation>,
    overwrite: Option<crate::files::DiskState>,
    recovery: Option<crate::recovery::Recovery>,
    recovered_sources: std::collections::BTreeMap<u64, PathBuf>,
    search_request: Option<(bool, Option<(String, bool)>)>,
    search_key: Option<search_refresh::Key>,
    search_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    search_task: Option<Task<()>>,
    popup_scroll: ScrollHandle,
    documentation_scroll: ScrollHandle,
    documentation_only: bool,
    popup_bounds: Vec<Bounds<Pixels>>,
    pub popup_frames: u64,
    popup_request: Option<u64>,
    #[cfg(feature = "perf")]
    popup_is_completion: bool,
    workspace_scroll: UniformListScrollHandle,
    popup_list_scroll: UniformListScrollHandle,

    services: super::services::Services,
    explorer: super::explorer::Explorer,
    _wake: crate::background::WakeGuard,
    closing: Option<u64>,
    quitting: bool,
    quit_when_clean: bool,
    quit_window: Option<WindowHandle<super::application::NativeWindow>>,
    destination: Option<(PathBuf, lsp_types::Position, crate::lsp::Encoding)>,
    reload: Option<(u64, u64)>,
    tool_busy: bool,
    typed_character: Option<char>,
    #[cfg(feature = "perf")]
    input_started: Option<std::time::Instant>,
    completion_identity: Option<(u64, u64, usize)>,
    resizing: bool,
    title_drag: Option<Point<Pixels>>,
    explorer_scroll: UniformListScrollHandle,
}
impl EditorView {
    pub fn new(model: Model, cx: &mut Context<Self>) -> Self {
        let theme = Theme::new(&model.editor.config);
        let toasts = cx.new(|_| super::toast_view::ToastView::new(&model.editor.config));
        let stage = cx.new(|cx| super::stage::Stage::new(&model.editor.config, theme, cx));
        cx.subscribe(
            &stage,
            |view, _, error: &super::stage::WallpaperError, cx| {
                view.toast(super::toast::Category::Error, "Wallpaper", &error.0, cx);
            },
        )
        .detach();
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (tx, rx) = async_channel::bounded(1);
        let wake = crate::background::WakeGuard::install(std::sync::Arc::new(move || {
            let _ = tx.try_send(());
        }));
        cx.spawn(async move |view, cx| {
            while rx.recv().await.is_ok() {
                if view
                    .update(cx, |view, cx| {
                        view.services.file_busy = view.workspace_file_busy();
                        let changed = view.services.poll(&mut view.model) | view.explorer.poll();
                        if view.services.workspace_changed {
                            view.services.workspace_changed = false;
                            view.explorer
                                .refresh(view.model.editor.config.navigation.clone());
                        }
                        view.workspace_confirmation(cx);
                        view.service_notifications(cx);
                        view.poll_recovery(cx);
                        if changed {
                            view.changed(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        let cursor = cx.new(|_| super::cursor_view::CursorView::new(focus.clone()));
        Self {
            manager_ui: ManagerUi::default(),
            toasts,
            cursor,
            services: super::services::Services::new(root.clone()),
            explorer: super::explorer::Explorer::new(root),
            _wake: wake,
            closing: None,
            quitting: false,
            quit_when_clean: false,
            quit_window: None,
            destination: None,
            reload: None,
            tool_busy: false,
            typed_character: None,
            #[cfg(feature = "perf")]
            input_started: None,
            completion_identity: None,
            resizing: false,
            title_drag: None,
            explorer_scroll: UniformListScrollHandle::new(),
            model: super::documents::Documents::new(model),
            theme,
            stage,
            clock: Default::default(),
            tab_lens: (
                super::motion::Motion::new(0., super::motion::Character::Glide),
                super::motion::Motion::new(0., super::motion::Character::Glide),
            ),
            tab_lens_ready: false,
            command_presence: super::motion::Motion::new(0., super::motion::Character::Glide),
            lens_frame: None,
            stage_layout: None,
            stage_mode: None,
            focus,
            bounds: None,
            cell: px(10.),
            gutter: GUTTER,
            cache: None,
            active_number: None,
            command_line: None,
            follow_cursor: true,
            busy: false,
            prompt: false,
            dragging: false,
            scroll_y: 0.,
            scroll_x: 0.,
            painted_frames: 0,
            shaped_rows: 0,
            reused_rows: 0,
            confirmation: None,
            overwrite: None,
            recovery: if cfg!(test) {
                None
            } else {
                Some(crate::recovery::Recovery::new(
                    crate::config::Config::path().with_file_name("recovery"),
                ))
            },
            recovered_sources: Default::default(),
            search_request: None,
            search_key: None,
            search_generation: Default::default(),
            search_task: None,
            popup_scroll: ScrollHandle::new(),
            documentation_scroll: ScrollHandle::new(),
            documentation_only: false,
            popup_bounds: Vec::new(),
            popup_frames: 0,
            popup_request: None,
            #[cfg(feature = "perf")]
            popup_is_completion: false,
            workspace_scroll: UniformListScrollHandle::new(),
            popup_list_scroll: UniformListScrollHandle::new(),
        }
    }
    fn workspace_file_busy(&self) -> bool {
        self.busy
            || self.prompt
            || self.quitting
            || self
                .confirmation
                .as_ref()
                .is_some_and(|c| !matches!(c.decision, Decision::Workspace { .. }))
    }
    fn input_enabled(&self) -> bool {
        !self.services.workspace_executing
            && !self.explorer.focused
            && !self.manager_ui.open
            && !self.prompt
            && !self.quitting
            && self.confirmation.is_none()
            && self.services.panel.as_ref().is_none_or(|p| {
                !matches!(
                    p.presentation,
                    super::popup::Presentation::Picker | super::popup::Presentation::ContextMenu
                )
            })
            && self.model.input_enabled()
    }
    fn toast(
        &mut self,
        category: super::toast::Category,
        title: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        self.toasts.update(cx, |toasts, cx| {
            toasts.show(
                super::toast::Notification::new(category, title, message),
                cx,
            );
        });
    }
    fn service_notifications(&mut self, cx: &mut Context<Self>) {
        while let Some(notification) = self.services.notifications.pop_front() {
            self.toasts.update(cx, |toasts, cx| {
                toasts.show(notification, cx);
            });
        }
    }
    fn dismiss_popup(&mut self) {
        self.documentation_only = false;
        self.services.dismiss();
        self.popup_bounds.clear();
    }
    fn accept_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .services
            .panel
            .as_ref()
            .is_some_and(|p| self.popup_request != Some(p.request))
        {
            return;
        }
        self.documentation_only = false;
        let accepting_completion = self
            .services
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == super::services::Kind::Completion);
        let revision = self.model.editor.buffer.revision;
        if let Some(choice) = self.services.accept(&mut self.model) {
            match choice {
                super::services::Choice::Location {
                    path,
                    position,
                    encoding,
                } => {
                    self.destination = Some((path.clone(), position, encoding));
                    self.request(
                        Request::Open {
                            path,
                            revision: self.model.editor.buffer.revision,
                        },
                        cx,
                    );
                }
                super::services::Choice::Buffer(id) => {
                    self.model.switch(id);
                }
                super::services::Choice::Command(command) => self.command(command, window, cx),
                _ => {}
            }
        }
        if accepting_completion && revision != self.model.editor.buffer.revision {
            // Acceptance is a programmatic edit, not another typed prefix.
            self.completion_identity = Some((
                self.model.id(),
                self.model.editor.buffer.revision,
                self.model.editor.buffer.cursor,
            ));
        }
        self.changed(cx);
    }
    fn line_height(&self) -> f32 {
        (self.model.editor.config.font_size * 1.45).ceil()
    }
    pub fn focus(&self, window: &mut Window, _cx: &mut App) {
        window.focus(&self.focus);
    }
    /// Exercises the same handlers as native events, only in explicit smoke mode.
    pub fn smoke_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.busy && self.model.editor.document_id > 0,
            "fixture has not loaded"
        );
        let initial = self.model.id();
        let second = self.model.add(
            crate::buffer::Buffer::new(
                ropey::Rope::from_str("fn second() { let café = 42; }"),
                Some(PathBuf::from("checkpoint-smoke.rs")),
            ),
            true,
        )?;
        self.model.editor.buffer.insert("// independent\n");
        self.model.act(Action::Undo);
        anyhow::ensure!(
            self.model
                .editor
                .buffer
                .text
                .to_string()
                .starts_with("fn second"),
            "second document undo failed"
        );
        self.model.switch(initial);
        anyhow::ensure!(
            self.model.get(second).is_some(),
            "switch lost second document"
        );
        let original = self.model.editor.buffer.text.clone();
        let press = |view: &mut Self,
                     key: &str,
                     window: &mut Window,
                     cx: &mut Context<Self>|
         -> anyhow::Result<()> {
            view.key(
                &KeyDownEvent {
                    keystroke: Keystroke::parse(key)?,
                    is_held: false,
                },
                window,
                cx,
            );
            Ok(())
        };
        press(self, "i", window, cx)?;
        anyhow::ensure!(
            self.model.editor.mode == Mode::Insert,
            "Insert binding failed"
        );
        self.replace_text_in_range(None, "Checkpoint 😀 e\u{301}\t", window, cx);
        anyhow::ensure!(
            self.model.editor.buffer.text != original,
            "text handler did not edit the real buffer"
        );
        let edited = self.model.editor.buffer.text.clone();
        press(self, "escape", window, cx)?;
        press(self, "u", window, cx)?;
        anyhow::ensure!(self.model.editor.buffer.text == original, "Undo failed");
        press(self, "ctrl-r", window, cx)?;
        anyhow::ensure!(self.model.editor.buffer.text == edited, "Redo failed");
        press(self, "u", window, cx)?;
        press(self, "v", window, cx)?;
        press(self, "d", window, cx)?;
        anyhow::ensure!(
            self.model.editor.selection().is_some_and(|r| !r.is_empty()),
            "Visual binding failed"
        );
        press(self, "escape", window, cx)?;
        // The fixture's third row is long, exercising horizontal visibility.
        self.model.editor.execute_command("3");
        press(self, "end", window, cx)?;
        window.resize(size(px(920.), px(640.)));
        self.changed(cx);
        Ok(())
    }
    pub fn smoke_geometry(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let e = &self.model.editor;
        let row = self
            .cache
            .as_ref()
            .and_then(|c| c.lines.iter().find(|r| r.source.row == e.buffer.row()))
            .ok_or_else(|| anyhow::anyhow!("cursor row missing from painted cache"))?;
        anyhow::ensure!(
            e.buffer.cursor >= row.source.start && e.buffer.cursor <= row.source.end,
            "cursor outside shaped fragment"
        );
        let x = row
            .shape
            .x_for_index(row.source.byte_for_char(e.buffer.cursor));
        let bounds = self
            .bounds
            .ok_or_else(|| anyhow::anyhow!("missing viewport geometry"))?;
        anyhow::ensure!(
            row.origin.x + x >= bounds.left() && row.origin.x + x <= bounds.right(),
            "shaped cursor outside viewport"
        );
        let at = self
            .char_at(point(row.origin.x + x, row.origin.y + px(2.)))
            .ok_or_else(|| anyhow::anyhow!("hit test failed"))?;
        anyhow::ensure!(at == e.buffer.cursor, "hit test / shaped cursor mismatch");
        let top = self.model.editor.top;
        let before = self.shaped_rows;
        let reused = self.reused_rows;
        self.follow_cursor = false;
        self.model.editor.top = top + 1;
        self.prepare(bounds, window, cx);
        anyhow::ensure!(
            self.shaped_rows - before <= 1 && self.reused_rows > reused,
            "scroll failed to reuse viewport shapes"
        );
        self.model.editor.top = top;
        self.prepare(bounds, window, cx);
        self.wheel(
            &ScrollWheelEvent {
                delta: ScrollDelta::Lines(point(0., -20.)),
                ..Default::default()
            },
            window,
            cx,
        );
        Ok(())
    }
    pub fn smoke_project(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let root = self
            .model
            .editor
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .ok_or_else(|| anyhow::anyhow!("fixture path unavailable"))?
            .to_path_buf();
        self.explorer
            .open(root.clone(), self.model.editor.config.navigation.clone());
        let request = self
            .model
            .editor
            .save(Some(root.join("saved-native.rs")), false)
            .ok_or_else(|| anyhow::anyhow!("save request unavailable"))?;
        self.request(request, cx);
        Ok(())
    }
    pub fn smoke_project_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.busy, "native save still pending");
        anyhow::ensure!(self.model.order.len() == 2, "native tab state lost");
        anyhow::ensure!(
            !self.model.editor.buffer.dirty(),
            "native save did not mark its snapshot saved"
        );
        anyhow::ensure!(
            self.explorer.index.is_some() && !self.explorer.rows.is_empty(),
            "explorer did not index fixture project"
        );
        self.explorer.width = 320.;
        window.resize(size(px(1100.), px(700.)));
        self.changed(cx);
        Ok(())
    }
    pub fn smoke_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let before = self.model.editor.buffer.text.clone();
        self.command("Commands".into(), window, cx);
        window.resize(size(px(640.), px(480.)));
        self.key(
            &KeyDownEvent {
                keystroke: Keystroke::parse("i")?,
                is_held: false,
            },
            window,
            cx,
        );
        anyhow::ensure!(
            self.model.editor.buffer.text == before,
            "picker key edited document"
        );
        anyhow::ensure!(
            self.services.panel.is_some(),
            "picker key escaped modal input policy"
        );
        self.changed(cx);
        Ok(())
    }
    pub fn smoke_popup_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.popup_frames > 0 && !self.popup_bounds.is_empty(),
            "floating popup was not prepainted"
        );
        let viewport = window.viewport_size();
        for bounds in &self.popup_bounds {
            anyhow::ensure!(
                bounds.left() >= px(0.)
                    && bounds.right() <= viewport.width
                    && bounds.top() >= px(0.)
                    && bounds.bottom() <= viewport.height,
                "popup outside viewport"
            );
        }
        self.key(
            &KeyDownEvent {
                keystroke: Keystroke::parse("escape")?,
                is_held: false,
            },
            window,
            cx,
        );
        anyhow::ensure!(
            self.services.panel.is_none(),
            "Escape did not dismiss picker"
        );
        self.model.editor.buffer.insert("temporary popup test");
        let id = self.model.id();
        self.close_document(id, window, cx);
        anyhow::ensure!(
            self.confirmation.is_some(),
            "dirty close did not create floating confirmation"
        );
        cx.notify();
        Ok(())
    }
    pub fn smoke_confirmation_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.confirmation.is_some(), "confirmation disappeared");
        self.key(
            &KeyDownEvent {
                keystroke: Keystroke::parse("escape")?,
                is_held: false,
            },
            window,
            cx,
        );
        anyhow::ensure!(
            self.confirmation.is_none() && self.model.editor.buffer.dirty(),
            "cancel lost document or failed to close confirmation"
        );
        self.model.act(Action::Undo);
        self.services.diagnostics_at(&self.model);
        self.changed(cx);
        Ok(())
    }
    pub fn smoke_information_ready(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.popup_bounds.is_empty(),
            "contextual information popup did not prepaint"
        );
        self.dismiss_popup();
        cx.notify();
        Ok(())
    }
    pub(super) fn set_lsp_smoke_storage(&mut self, data: PathBuf) {
        self.services.manager = crate::lsp_manager::Manager::at(data, self.services.root.clone());
    }

    /// Explicit bounded real-server smoke mode; trust is stored in temporary test data.
    pub fn lsp_smoke_step(
        &mut self,
        stage: u8,
        tick: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<u8> {
        use super::services::Kind;
        let press = |v: &mut Self,
                     key: &str,
                     w: &mut Window,
                     cx: &mut Context<Self>|
         -> anyhow::Result<()> {
            v.key(
                &KeyDownEvent {
                    keystroke: Keystroke::parse(key)?,
                    is_held: false,
                },
                w,
                cx,
            );
            Ok(())
        };
        match stage {
            0 => {
                let manager = &mut self.services.manager;
                if self.model.editor.buffer.path.is_some()
                    && manager.loaded
                    && !manager.snapshot.trusted
                    && !manager.busy
                {
                    anyhow::ensure!(
                        manager.error.is_none(),
                        "Cannot trust smoke fixture: {:?}",
                        manager.error
                    );
                    manager.submit(crate::lsp_manager::Operation::TrustWorkspace(true));
                }
                if self.busy || !self.services.completion_ready(&self.model) {
                    return Ok(0);
                }
                let text = self.model.editor.buffer.text.to_string();
                let byte = text
                    .find("widget.com")
                    .ok_or_else(|| anyhow::anyhow!("LSP smoke fixture needs widget.com"))?
                    + "widget.".len();
                let start = text[..byte].chars().count();
                self.model.editor.set_mode(Mode::Insert);
                let range = self.model.to_utf16(start)..self.model.to_utf16(start + 3);
                self.replace_text_in_range(Some(range), "co", window, cx);
                Ok(1)
            }
            1 => {
                if let Some(panel) = &mut self.services.panel
                    && panel.kind == Kind::Completion
                    && let Some(i) = panel
                        .rows
                        .iter()
                        .position(|r| r.label.starts_with("compute"))
                {
                    panel.selected = i;
                    self.services.selection_changed(&self.model);
                    cx.notify();
                    return Ok(2);
                }
                if tick.is_multiple_of(30) {
                    let text = self.model.editor.buffer.text.to_string();
                    if let Some(byte) = text.find("widget.co") {
                        let start = text[..byte + "widget.".len()].chars().count();
                        let end = self.model.editor.buffer.cursor;
                        let replacement = if end - start == 2 { "com" } else { "co" };
                        let range = self.model.to_utf16(start)..self.model.to_utf16(end);
                        self.replace_text_in_range(Some(range), replacement, window, cx);
                    }
                }
                Ok(1)
            }
            2 => {
                anyhow::ensure!(
                    !self.popup_bounds.is_empty(),
                    "real completion dropdown did not prepaint"
                );
                window.resize(size(px(620.), px(380.)));
                press(self, "f1", window, cx)?;
                Ok(20)
            }
            20 => {
                if self.services.resolving_selected() {
                    return Ok(20);
                }
                anyhow::ensure!(
                    self.documentation_only && !self.popup_bounds.is_empty(),
                    "completion documentation did not prepaint"
                );
                anyhow::ensure!(
                    self.services
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.rows[p.selected].detail.contains("Computes")),
                    "real completion documentation unavailable"
                );
                press(self, "f1", window, cx)?;
                press(self, "enter", window, cx)?;
                Ok(3)
            }
            3 => {
                let text = self.model.editor.buffer.text.to_string();
                let Some(byte) = text.find("widget.compute(") else {
                    return Ok(3);
                };
                anyhow::ensure!(
                    self.model.editor.mode == Mode::Insert && !text.contains("${"),
                    "completion violated mode or inserted raw snippet syntax"
                );
                press(self, "escape", window, cx)?;
                self.model.editor.buffer.cursor = text[..byte + "widget.co".len()].chars().count();
                self.changed(cx);
                self.command("Hover".into(), window, cx);
                Ok(4)
            }
            4 => {
                if self.services.panel.as_ref().is_some_and(|p| {
                    p.kind == Kind::Hover && p.rows.iter().any(|r| r.label.contains("Computes"))
                }) {
                    return Ok(5);
                }
                // rust-analyzer can reject the first hover while indexing the
                // just-accepted completion. Retry only that transient response.
                if tick.is_multiple_of(5) && self.model.editor.message.contains("-32801") {
                    self.model.editor.message.clear();
                    self.command("Hover".into(), window, cx);
                }
                Ok(4)
            }
            5 => {
                if !self
                    .model
                    .editor
                    .buffer
                    .text
                    .to_string()
                    .contains("_wrong = (")
                {
                    anyhow::ensure!(!self.popup_bounds.is_empty(), "real hover did not prepaint");
                }
                if !self.services.diagnostics.values().any(|(rev, _, items)| {
                    *rev == self.model.editor.buffer.revision && !items.is_empty()
                }) {
                    let text = self.model.editor.buffer.text.to_string();
                    if let Some(byte) = text.find("_wrong = ;") {
                        let at = text[..byte + "_wrong = ".len()].chars().count();
                        self.model.editor.buffer.cursor = at;
                        self.model.editor.set_mode(Mode::Insert);
                        self.replace_text_in_range(None, "(", window, cx);
                        press(self, "escape", window, cx)?;
                    }
                    return Ok(5);
                }
                let text = self.model.editor.buffer.text.to_string();
                let byte = text
                    .find("_wrong")
                    .ok_or_else(|| anyhow::anyhow!("fixture needs _wrong diagnostic marker"))?;
                self.model.editor.buffer.cursor = text[..byte].chars().count();
                self.changed(cx);
                self.services.diagnostics_at(&self.model);
                cx.notify();
                Ok(6)
            }
            6 => {
                anyhow::ensure!(
                    !self.popup_bounds.is_empty()
                        && self
                            .services
                            .panel
                            .as_ref()
                            .is_some_and(|p| p.rows.iter().any(|r| r.label.contains("[Error]"))),
                    "real diagnostic popup missing"
                );
                self.dismiss_popup();
                self.command("Commands".into(), window, cx);
                Ok(7)
            }
            7 => {
                anyhow::ensure!(
                    !self.popup_bounds.is_empty(),
                    "floating command picker missing"
                );
                let before = self.model.editor.buffer.text.clone();
                press(self, "down", window, cx)?;
                press(self, "escape", window, cx)?;
                anyhow::ensure!(
                    self.model.editor.buffer.text == before && self.services.panel.is_none(),
                    "picker dismissal changed text"
                );
                self.services.shutdown();
                Ok(8)
            }
            8 => Ok(if self.services.stopped() { 9 } else { 8 }),
            _ => Ok(9),
        }
    }
    pub fn smoke_shutdown(&mut self, cx: &mut Context<Self>) {
        self.finish_quit(cx);
    }
    pub fn smoke_cursor_motion(
        &mut self,
        tick: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.dismiss_popup();
        if tick == 0 {
            self.model.editor.set_mode(Mode::Normal);
            self.model.editor.buffer.cursor = self
                .model
                .editor
                .buffer
                .text
                .line_to_char(10.min(self.model.editor.buffer.text.len_lines() - 1));
            self.changed(cx);
            return Ok(());
        }
        self.key(
            &KeyDownEvent {
                keystroke: Keystroke::parse(if tick % 16 < 8 { "right" } else { "left" })?,
                is_held: false,
            },
            window,
            cx,
        );
        Ok(())
    }
    pub fn smoke_summary(&self, cx: &App) -> String {
        format!(
            "painted_frames={} document_id={} chars={} lines={} cached_rows={} busy={} error={} message={} shaped_rows={} reused_rows={} cursor_frames={} cursor_active={}\n",
            self.painted_frames,
            self.model.editor.document_id,
            self.model.editor.buffer.text.len_chars(),
            self.model.editor.buffer.text.len_lines(),
            self.cache.as_ref().map_or(0, |c| c.lines.len()),
            self.busy,
            self.model.editor.error,
            self.model.editor.message,
            self.shaped_rows,
            self.reused_rows,
            self.cursor.read(cx).frames,
            self.cursor.read(cx).active()
        )
    }
    pub fn open_initial(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                (path.is_dir(), crate::paths::project_root(&path), path)
            }
        });
        cx.spawn(async move |view, cx| {
            let (directory, root, path) = task.await;
            let _ = view.update(cx, |v, cx| {
                v.services.root = root.clone();
                v.explorer.root = root;
                if directory {
                    v.services.restart();
                    v.services.root = path.clone();
                    v.explorer
                        .open(path, v.model.editor.config.navigation.clone());
                    v.changed(cx);
                } else {
                    v.request(
                        Request::Open {
                            path,
                            revision: v.model.editor.buffer.revision,
                        },
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Reconcile after input or an applied result; completion follows logical identity.
    fn poll_recovery(&mut self, cx: &mut Context<Self>) {
        if self.confirmation.is_some() {
            return;
        }
        let events: Vec<_> = self
            .recovery
            .as_ref()
            .map(|r| r.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                crate::recovery::Event::Available(entries) => {
                    self.confirmation = Some(Confirmation {
                        decision: Decision::Recovery(entries),
                        document: self.model.id(),
                        revision: self.model.editor.buffer.revision,
                        selected: 0,
                    });
                    self.dismiss_popup();
                    cx.notify();
                }
                crate::recovery::Event::Error(message) => {
                    self.toast(super::toast::Category::Error, "Recovery", &message, cx)
                }
            }
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.services.file_busy = self.workspace_file_busy();
        self.workspace_confirmation(cx);
        self.refresh_search(cx);
        if let Some(recovery) = &mut self.recovery {
            recovery.sync(&self.model);
            let mut obsolete = Vec::new();
            self.recovered_sources.retain(|id, path| {
                if self.model.get(*id).is_none_or(|e| !e.buffer.dirty()) {
                    obsolete.push(path.clone());
                    false
                } else {
                    true
                }
            });
            if !obsolete.is_empty() {
                recovery.discard_paths(obsolete);
            }
        }
        self.poll_recovery(cx);
        if let Some((path, position, encoding)) = &self.destination
            && self
                .model
                .editor
                .buffer
                .path
                .as_ref()
                .is_some_and(|p| crate::lsp::path_key(p) == crate::lsp::path_key(path))
        {
            if let Ok(at) = crate::lsp::offset(&self.model.editor.buffer.text, *position, *encoding)
            {
                self.model.editor.buffer.cursor = at;
                self.model.editor.buffer.snap_cursor();
            }
            self.destination = None;
        }
        self.follow_cursor = true;
        self.services.update(&self.model);
        self.service_notifications(cx);
        if let Some(message) = self.services.message.take() {
            self.model.editor.notify(message, false);
        }
        let identity = (
            self.model.id(),
            self.model.editor.buffer.revision,
            self.model.editor.buffer.cursor,
        );
        if self.completion_identity != Some(identity) {
            self.completion_identity = Some(identity);
            if self.model.marked.is_none() {
                self.services.automatic_completion(&self.model);
            }
        }
        cx.notify();
    }
    /// Dispatch side effects, serializing file/search jobs while edits remain possible.
    /// Results return to their owning document; reload/search also validate revision.
    fn request(&mut self, mut request: Request, cx: &mut Context<Self>) {
        if matches!(request, Request::Search { .. }) {
            let action = self.search_request.take().unwrap_or((false, None));
            self.start_search(request, action, false, cx);
            return;
        }
        // Reject before changing close/reload intent: a second save must not
        // attach a close request to the operation already in flight.
        if (self.busy || self.services.workspace_active())
            && !matches!(request, Request::Clipboard(_))
        {
            self.model.editor.notify(
                "A file/search operation is in progress; retry when it completes",
                true,
            );
            cx.notify();
            return;
        }
        match &mut request {
            Request::Open { path, .. } | Request::Save { path, .. } => {
                *path = crate::paths::absolute(&self.services.root, path)
            }
            _ => {}
        }
        if let Request::Open { path, .. } = &request
            && self.reload.is_none()
            && let Some(id) = self.model.find(path)
        {
            self.model.switch(id);
            self.changed(cx);
            return;
        }
        if let Request::Save { path, quit, .. } = &request {
            if self
                .model
                .find(path)
                .is_some_and(|id| id != self.model.id())
            {
                self.model.editor.notify(
                    "That path belongs to another open document; close it before Save As",
                    true,
                );
                return;
            }
            if *quit {
                self.quit_when_clean = self.model.order.len() == 1;
                self.closing = Some(self.model.id());
            }
        }
        match request {
            Request::Clipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Request::Quit => {
                if self.busy {
                    self.model
                        .editor
                        .notify("Wait for the file operation before closing", true);
                    cx.notify();
                } else {
                    if self.model.order.len() == 1 {
                        self.finish_quit(cx);
                        return;
                    }
                    let id = self.model.id();
                    let _ = self.model.close_document(id, false);
                    self.changed(cx);
                }
            }
            r => {
                if self.busy {
                    self.model.editor.notify(
                        "A file/search operation is already in progress; retry when it completes",
                        true,
                    );
                    cx.notify();
                    return;
                }
                let Some(mut job) = io::Job::from_request(r, self.model.editor.document_id) else {
                    return;
                };
                if let io::Job::Save { path, expected, .. } = &mut job {
                    let buffer = &self.model.editor.buffer;
                    if buffer
                        .path
                        .as_ref()
                        .is_some_and(|p| crate::lsp::path_key(p) == crate::lsp::path_key(path))
                    {
                        *expected = buffer
                            .disk_state
                            .clone()
                            .unwrap_or(crate::files::DiskState::Missing);
                    }
                }
                if let io::Job::Save { protected, .. } = &mut job {
                    *protected = self
                        .model
                        .order
                        .iter()
                        .filter(|id| **id != self.model.id())
                        .filter_map(|id| self.model.get(*id).and_then(|e| e.buffer.path.clone()))
                        .collect();
                }
                if let io::Job::Save { expected, .. } = &mut job
                    && let Some(approved) = self.overwrite.take()
                {
                    *expected = approved;
                }
                self.busy = true;
                let task = cx.background_executor().spawn(async move { job.run() });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    let _ = view.update(cx, |view, cx| {
                        view.busy = false;
                        match result {
                            Ok(done) => match done {
                                io::Done::Conflict {
                                    path,
                                    actual,
                                    document,
                                } => {
                                    view.quit_when_clean = false;
                                    view.closing = None;
                                    if let Some(editor) = view.model.get(document) {
                                        let revision = editor.buffer.revision;
                                        view.confirmation = Some(Confirmation {
                                            document,
                                            revision,
                                            selected: 1,
                                            decision: Decision::Conflict {
                                                path,
                                                actual,
                                                overwrite: false,
                                            },
                                        });
                                        view.dismiss_popup();
                                    }
                                }
                                io::Done::Open {
                                    path,
                                    text,
                                    disk_state,
                                    indentation,
                                    ..
                                } => {
                                    let mut buffer = crate::buffer::Buffer::new(text, Some(path));
                                    buffer.disk_state = Some(disk_state);
                                    buffer.indentation = indentation;
                                    if let Some((id, revision)) = view.reload.take() {
                                        if let Err(error) = view.model.reload(id, revision, buffer)
                                        {
                                            view.model.editor.notify(error.to_string(), true);
                                        }
                                    } else if let Err(error) = view.model.add(buffer, true) {
                                        view.model.editor.notify(error.to_string(), true);
                                    }
                                    view.cache = None;
                                }
                                done => {
                                    let id = match &done {
                                        io::Done::Save { document, .. }
                                        | io::Done::Search { document, .. } => *document,
                                        _ => unreachable!(),
                                    };
                                    let saved = matches!(&done, io::Done::Save { .. });
                                    if matches!(&done, io::Done::Search { .. }) {
                                        view.cache = None;
                                    }
                                    if let Some(editor) = view.model.get_mut(id) {
                                        io::apply(editor, done);
                                    }
                                    if saved {
                                        view.services.saved(&view.model, id);
                                    }
                                    if view.closing == Some(id) {
                                        view.closing = None;
                                        if view.model.get(id).is_some_and(|e| !e.buffer.dirty()) {
                                            let _ = view.model.close_document(id, false);
                                        }
                                    }
                                }
                            },
                            Err(error) => {
                                view.quit_when_clean = false;
                                view.reload = None;
                                view.destination = None;
                                view.closing = None;
                                view.toast(
                                    super::toast::Category::Error,
                                    "File operation failed",
                                    &format!("{error:#}"),
                                    cx,
                                );
                                view.model.editor.notify(format!("{error:#}"), true);
                            }
                        }
                        view.continue_quit(cx);
                        view.changed(cx);
                    });
                })
                .detach();
            }
        }
    }
    fn save(&mut self, quit: bool, force_path: bool, cx: &mut Context<Self>) {
        if self.model.editor.buffer.path.is_some() && !force_path {
            if let Some(r) = self.model.editor.save(None, quit) {
                self.request(r, cx);
            }
            return;
        }
        if self.prompt || self.busy {
            return;
        }
        self.prompt = true;
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let document = self.model.editor.document_id;
        let receiver = cx.prompt_for_new_path(&directory, Some("untitled.txt"));
        cx.spawn(async move |view, cx| {
            let result = receiver.await;
            let _ = view.update(cx, |view, cx| {
                view.prompt = false;
                match result {
                    Ok(Ok(Some(path))) if view.model.editor.document_id == document => {
                        if let Some(r) = view.model.editor.save(Some(path), quit) {
                            view.request(r, cx);
                        }
                    }
                    Ok(Err(e)) => view.model.editor.notify(format!("{e:#}"), true),
                    _ => {
                        view.closing = None;
                        view.quit_when_clean = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn open_picker(&mut self, cx: &mut Context<Self>) {
        if self.prompt || self.busy {
            return;
        }
        self.prompt = true;
        let document = self.model.editor.document_id;
        let revision = self.model.editor.buffer.revision;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |view, cx| {
            let result = receiver.await;
            let _ = view.update(cx, |view, cx| {
                view.prompt = false;
                match result {
                    Ok(Ok(Some(paths)))
                        if view.model.editor.document_id == document
                            && view.model.editor.buffer.revision == revision =>
                    {
                        if let Some(path) = paths.into_iter().next() {
                            view.request(Request::Open { path, revision }, cx);
                        }
                    }
                    Ok(Err(e)) => view.model.editor.notify(format!("{e:#}"), true),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn finish_quit(&mut self, cx: &mut Context<Self>) {
        if self.services.workspace_active() {
            self.model
                .editor
                .notify("Finish or cancel the workspace edit before closing", true);
            return;
        }
        if self.quitting {
            return;
        }
        self.quitting = true;
        let recovery_flush = self.recovery.as_mut().map(|recovery| {
            recovery.sync(&self.model);
            recovery.flush()
        });
        let recovery_task = cx.background_executor().spawn(async move {
            if let Some(flush) = recovery_flush {
                let _ = flush.recv();
            }
        });
        self.services.shutdown();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |view, cx| {
            for tick in 0..100 {
                executor.timer(std::time::Duration::from_millis(50)).await;
                let (stopped, installing) = view
                    .update(cx, |v, _| (v.services.stopped(), v.services.manager.busy))
                    .unwrap_or((true, false));
                if stopped || tick >= 29 && !installing {
                    break;
                }
            }
            recovery_task.await;
            let _ = view.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
    fn continue_quit(&mut self, cx: &mut Context<Self>) {
        if !self.quit_when_clean || self.prompt || self.busy {
            return;
        }
        if self.model.dirty_ids().is_empty() {
            self.finish_quit(cx);
            return;
        }
        if let Some(window) = self.quit_window.take() {
            cx.spawn(async move |_, cx| {
                let _ = window.update(cx, |root, window, cx| {
                    root.with_editor(window, cx, |view, window, cx| view.close(window, cx));
                });
            })
            .detach();
        }
    }
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.manager_ui.open = false;
        self.quit_when_clean = true;
        self.quit_window = window.window_handle().downcast();
        if self.model.dirty_ids().is_empty() && !self.busy {
            self.finish_quit(cx);
            return false;
        }
        if let Some(id) = self.model.dirty_ids().first().copied() {
            self.model.switch(id);
            self.close_document(id, window, cx);
        }
        false
    }
    fn close_document(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || self.services.workspace_active()
            || self.prompt
            || self.confirmation.is_some()
        {
            return;
        }
        let Some(editor) = self.model.get(id) else {
            return;
        };
        if !editor.buffer.dirty() {
            let _ = self.model.close_document(id, false);
            self.changed(cx);
            return;
        }
        let revision = editor.buffer.revision;
        self.dismiss_popup();
        self.confirmation = Some(Confirmation {
            decision: Decision::Close,
            document: id,
            revision,
            selected: 2,
        });
        cx.notify();
    }
    fn workspace_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.confirmation.is_none()
            && !self.busy
            && let Some(approval) = &self.services.workspace_approval
        {
            let token = approval.token;
            self.dismiss_popup();
            self.manager_ui.open = false;
            self.confirmation = Some(Confirmation {
                decision: Decision::Workspace {
                    token,
                    expanded: false,
                    position: 0,
                },
                document: self.model.id(),
                revision: self.model.editor.buffer.revision,
                selected: 1,
            });
            cx.notify();
        }
    }
    fn answer_close(&mut self, answer: usize, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirmation.take() else {
            return;
        };
        if let Decision::Workspace { token, .. } = confirm.decision {
            self.services.file_busy = self.workspace_file_busy();
            self.services
                .workspace_answer(&self.model, token, answer == 0);
            self.changed(cx);
            return;
        }
        if let Decision::Recovery(entries) = confirm.decision {
            if answer == 0 {
                for entry in entries {
                    let path = entry.path.filter(|p| self.model.find(p).is_none());
                    let mut buffer = crate::buffer::Buffer::new(ropey::Rope::new(), path);
                    buffer.disk_state = entry.baseline;
                    buffer.insert(&entry.text);
                    buffer.mark_recovered();
                    buffer.cursor = entry.cursor.min(buffer.text.len_chars());
                    buffer.snap_cursor();
                    match self.model.add(buffer, true) {
                        Ok(id) => {
                            if let Some(recovery) = &mut self.recovery {
                                recovery.adopt(id, entry.source.clone());
                            }
                            self.recovered_sources.insert(id, entry.source);
                        }
                        Err(error) => self
                            .model
                            .editor
                            .notify(format!("Recovery failed: {error:#}"), true),
                    }
                }
                self.changed(cx);
            } else if answer == 1 {
                if let Some(recovery) = &self.recovery {
                    recovery.discard(&entries);
                }
                cx.notify();
            } else {
                cx.notify();
            }
            return;
        }
        if self
            .model
            .get(confirm.document)
            .is_none_or(|e| e.buffer.revision != confirm.revision)
        {
            self.quit_when_clean = false;
            self.model
                .editor
                .notify("Document changed; close it again to confirm", true);
            cx.notify();
            return;
        }
        if let Decision::Conflict {
            path,
            actual,
            overwrite,
        } = confirm.decision
        {
            self.model.switch(confirm.document);
            if overwrite {
                if answer == 0 {
                    self.overwrite = Some(actual);
                    if let Some(request) = self.model.editor.save(Some(path), false) {
                        self.request(request, cx);
                    }
                }
            } else {
                match answer {
                    0 => {
                        self.reload = Some((confirm.document, confirm.revision));
                        self.request(
                            Request::Open {
                                path,
                                revision: confirm.revision,
                            },
                            cx,
                        );
                    }
                    2 => self.save(false, true, cx),
                    3 => {
                        self.confirmation = Some(Confirmation {
                            document: confirm.document,
                            revision: confirm.revision,
                            selected: 1,
                            decision: Decision::Conflict {
                                path,
                                actual,
                                overwrite: true,
                            },
                        })
                    }
                    _ => {}
                }
            }
            cx.notify();
            return;
        }
        match answer {
            0 => {
                self.model.switch(confirm.document);
                self.closing = Some(confirm.document);
                self.save(false, false, cx);
            }
            1 => {
                let _ = self.model.close_document(confirm.document, true);
                self.continue_quit(cx);
                self.changed(cx);
            }
            _ => {
                self.quit_when_clean = false;
                cx.notify();
            }
        }
    }
    fn project_picker(&mut self, cx: &mut Context<Self>) {
        if self.prompt {
            return;
        }
        self.prompt = true;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |view, cx| {
            let result = receiver.await;
            let _ = view.update(cx, |v, cx| {
                v.prompt = false;
                if let Ok(Ok(Some(paths))) = result
                    && let Some(root) = paths.into_iter().next()
                {
                    v.services.restart();
                    v.services.root = root.clone();
                    v.explorer
                        .open(root, v.model.editor.config.navigation.clone());
                    v.changed(cx);
                }
            });
        })
        .detach();
    }
    fn setting(&mut self, key: String, value: String, cx: &mut Context<Self>) {
        if self.tool_busy {
            return;
        }
        self.tool_busy = true;
        let appearance =
            key.starts_with("theme.") || key == "theme_name" || key.starts_with("background.");
        let visual_only =
            appearance || key.starts_with("cursor.") || key.starts_with("completion.");
        let task = cx.background_executor().spawn(async move {
            crate::setup::save_setting(&crate::config::Config::path(), &key, &value)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.tool_busy = false;
                match result.and_then(|config| {
                    v.model.configure(config.clone())?;
                    Ok(config)
                }) {
                    Ok(config) => {
                        v.theme = Theme::new(&config);
                        v.toasts
                            .update(cx, |toasts, cx| toasts.configure(&config, cx));
                        if appearance {
                            v.cache = None;
                            v.command_line = None;
                            v.stage.update(cx, |stage, cx| {
                                stage.configure(&config, v.theme, false, cx)
                            });
                        }
                        if !visual_only {
                            v.cache = None;
                            v.services.restart();
                        }
                        if appearance {
                            v.toast(
                                super::toast::Category::Success,
                                "Appearance",
                                "Appearance settings saved",
                                cx,
                            );
                        } else {
                            v.model.editor.notify("Setting saved", false);
                        }
                    }
                    Err(e) if appearance => v.toast(
                        super::toast::Category::Error,
                        "Appearance",
                        &format!("{e:#}"),
                        cx,
                    ),
                    Err(e) => v.model.editor.notify(format!("{e:#}"), true),
                }
                v.changed(cx);
            });
        })
        .detach();
    }
    fn project_search(&mut self, query: String, grep: bool, cx: &mut Context<Self>) {
        if self.tool_busy {
            return;
        }
        let Some(index) = self.explorer.index.clone() else {
            self.model.editor.notify(
                "Open a project and wait for its index before :Files or :Grep",
                true,
            );
            return;
        };
        let config = self.model.editor.config.navigation.clone();
        let document = self.model.id();
        let revision = self.model.editor.buffer.revision;
        self.tool_busy = true;
        let task = cx.background_executor().spawn(async move {
            if grep {
                crate::navigation::grep(&index, &query, &config, || false)
            } else {
                crate::navigation::SearchResults {
                    matches: crate::navigation::rank(
                        &index.files,
                        &query,
                        config.max_results,
                        || false,
                    ),
                    message: String::new(),
                }
            }
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.tool_busy = false;
                if v.model.id() != document || v.model.editor.buffer.revision != revision {
                    return;
                }
                let rows = result
                    .matches
                    .into_iter()
                    .filter_map(|m| {
                        m.item.path().map(|path| super::services::Row {
                            label: format!("{} {}", m.item.label, m.item.detail),
                            detail: String::new(),
                            choice: super::services::Choice::Location {
                                path: path.to_path_buf(),
                                position: lsp_types::Position::new(
                                    m.item.line as u32,
                                    m.item.column as u32,
                                ),
                                encoding: crate::lsp::Encoding::Utf32,
                            },
                        })
                    })
                    .collect();
                v.services
                    .show(&v.model, if grep { "Project grep" } else { "Files" }, rows);
                v.model.editor.notify(result.message, false);
                cx.notify();
            });
        })
        .detach();
    }
    fn git(&mut self, name: &str, arg: &str, cx: &mut Context<Self>) {
        if self.tool_busy {
            return;
        }
        if !self.model.editor.config.git.enabled {
            self.model
                .editor
                .notify("Git is disabled in configuration", true);
            return;
        }
        if name == "GitStage" && self.model.editor.buffer.dirty() {
            self.model.editor.notify(
                "Save this document before staging its on-disk contents",
                true,
            );
            return;
        }
        let root = self.services.root.clone();
        let e = &self.model.editor;
        let path = e.buffer.path.clone();
        let document = self.model.id();
        let revision = e.buffer.revision;
        let job = match name {
            "GitStatus" => crate::git::Job::Refresh {
                root,
                path,
                text: Some(e.buffer.text.clone()),
                revision,
            },
            "GitCommit" => crate::git::Job::Commit {
                root,
                message: arg.into(),
            },
            _ => {
                let Some(path) = path else {
                    self.model.editor.notify("Save this document first", true);
                    return;
                };
                if name == "GitDiff" {
                    crate::git::Job::Diff {
                        root,
                        path,
                        staged: arg == "staged",
                    }
                } else {
                    crate::git::Job::Stage {
                        root,
                        path,
                        undo: name == "GitUnstage",
                    }
                }
            }
        };
        self.tool_busy = true;
        let task = cx
            .background_executor()
            .spawn(async move { crate::git::execute(job).map(super::services::git_rows) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.tool_busy = false;
                match result {
                    Ok((title, rows))
                        if v.model.id() == document
                            && v.model.editor.buffer.revision == revision =>
                    {
                        if rows.is_empty() {
                            v.model.editor.notify(title, false);
                        } else {
                            v.services.show(&v.model, &title, rows);
                        }
                    }
                    Ok(_) => v.model.editor.notify(
                        "Git job completed; document changed, reopen Git results to refresh",
                        false,
                    ),
                    Err(e) => v.model.editor.notify(format!("{e:#}"), true),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn document_search(
        &mut self,
        backwards: bool,
        replacement: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) {
        self.search_request = Some((backwards, replacement.clone()));
        let editor = &self.model.editor;
        let start = if backwards || replacement.is_some() || editor.search_ranges.is_empty() {
            editor.buffer.cursor
        } else {
            editor.buffer.next(editor.buffer.cursor)
        };
        self.request(
            Request::Search {
                text: editor.buffer.text.clone(),
                query: editor.last_search.clone(),
                start,
                revision: editor.buffer.revision,
            },
            cx,
        );
    }
    fn command(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let (name, arg) = command
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((command.trim(), ""));
        let arg = arg.trim();
        let arg = if arg.len() >= 2
            && ((arg.starts_with('"') && arg.ends_with('"'))
                || (arg.starts_with('\'') && arg.ends_with('\'')))
        {
            &arg[1..arg.len() - 1]
        } else {
            arg
        };
        if self.busy && matches!(name, "q!" | "bd!") {
            self.model.editor.notify(
                "Wait for the pending file operation before discarding this document",
                true,
            );
            return;
        }
        let mut action_name = String::new();
        for (index, ch) in name.chars().enumerate() {
            if ch.is_uppercase() && index > 0 {
                action_name.push('_');
            }
            action_name.extend(ch.to_lowercase());
        }
        if let Ok(action) = Action::parse(&action_name)
            && let Some(reason) = action.unavailable_reason()
        {
            self.model.editor.notify(reason, true);
            return;
        }
        use super::services::Kind;
        match name {
            "Comment" | "comment" => {
                self.model.editor.toggle_comment();
                self.changed(cx);
                return;
            }
            "Indent" | "indent" => {
                self.model.editor.indent_lines(false);
                self.changed(cx);
                return;
            }
            "Unindent" | "unindent" => {
                self.model.editor.indent_lines(true);
                self.changed(cx);
                return;
            }
            "find" | "find-regex" => {
                self.model.editor.last_search = arg.into();
                self.model.editor.search_ranges.clear();
                self.model.editor.search_options.regex = name == "find-regex";
                self.document_search(false, None, cx);
                return;
            }
            "find-next" | "find-prev" => {
                self.document_search(name == "find-prev", None, cx);
                return;
            }
            "replace" | "replace-all" => {
                self.document_search(false, Some((arg.into(), name == "replace-all")), cx);
                return;
            }
            "search-case" | "search-word" | "search-regex" => {
                if !matches!(arg, "on" | "off") {
                    self.model.editor.notify("Use on or off", true);
                    return;
                }
                let value = arg == "on";
                match name {
                    "search-case" => self.model.editor.search_options.ignore_case = !value,
                    "search-word" => self.model.editor.search_options.whole_word = value,
                    _ => self.model.editor.search_options.regex = value,
                }
                self.document_search(false, None, cx);
                return;
            }
            "search-clear" => {
                self.model.editor.last_search.clear();
                self.model.editor.search_ranges.clear();
                self.model.editor.notify("Search cleared", false);
                self.cache = None;
                self.changed(cx);
                return;
            }

            #[cfg(feature = "perf")]
            "CompletionPerf" => {
                let rows = crate::perf::report()
                    .lines()
                    .map(|line| super::services::Row {
                        label: line.into(),
                        detail: String::new(),
                        choice: super::services::Choice::Text,
                    })
                    .collect();
                self.services
                    .show(&self.model, "Completion performance (microseconds)", rows);
            }
            #[cfg(feature = "perf")]
            "CompletionPerfReset" => {
                crate::perf::reset();
                self.model.editor.notify("Performance samples reset", false);
            }
            "BackgroundReload" => {
                let config = self.model.editor.config.clone();
                self.stage.update(cx, |stage, cx| {
                    stage.configure(&config, self.theme, true, cx)
                });
            }
            "LspManager" | "LspInstall" => {
                self.open_manager(cx);
                return;
            }
            "LspSettings" => {
                match arg.split_once(char::is_whitespace).and_then(|(id, value)| {
                    serde_json::from_str(value)
                        .ok()
                        .map(|settings| (id, settings))
                }) {
                    Some((id, settings)) => {
                        self.services
                            .manager
                            .submit(crate::lsp_manager::Operation::Configure {
                                workspace: true,
                                change: crate::lsp_manager::Change::Settings(id.into(), settings),
                            });
                    }
                    None => self
                        .model
                        .editor
                        .notify("Use :LspSettings server-id {\"section\":{...}}", true),
                }
                return;
            }
            "LspPath" => {
                if let Some((id, path)) = arg.split_once(char::is_whitespace) {
                    if let Some(entry) = crate::lsp_catalog::entry(id) {
                        self.services
                            .manager
                            .submit(crate::lsp_manager::Operation::Configure {
                                workspace: true,
                                change: crate::lsp_manager::Change::Custom(
                                    id.into(),
                                    crate::lsp_manager::Launch {
                                        command: path.trim().trim_matches('"').into(),
                                        args: entry.args.iter().map(|s| s.to_string()).collect(),
                                    },
                                ),
                            });
                    } else {
                        self.model
                            .editor
                            .notify("Unknown catalog server identifier", true);
                    }
                } else {
                    self.model
                        .editor
                        .notify("Use :LspPath server-id absolute-executable-path", true);
                }
                self.open_manager(cx);
                return;
            }
            "set" => {
                if let Some((key, value)) = arg.split_once('=') {
                    self.setting(key.trim().into(), value.trim().into(), cx);
                } else {
                    self.model
                        .editor
                        .notify("Use :set key=value (for example font_size=20)", true);
                }
                return;
            }
            "new" | "enew" | "new!" => {
                let _ = self.model.add(crate::buffer::Buffer::default(), true);
            }
            "Buffers" => {
                let rows = self
                    .model
                    .order
                    .iter()
                    .map(|id| {
                        let e = self.model.get(*id).unwrap();
                        super::services::Row {
                            label: format!(
                                "{}{}",
                                e.buffer
                                    .path
                                    .as_ref()
                                    .map_or_else(|| "Untitled".into(), |p| p.display().to_string()),
                                if e.buffer.dirty() { " [+]" } else { "" }
                            ),
                            detail: String::new(),
                            choice: super::services::Choice::Buffer(*id),
                        }
                    })
                    .collect();
                self.services.show(&self.model, "Buffers", rows);
            }
            "Commands" => {
                let rows = [
                    "OpenProject",
                    "Explorer",
                    "Buffers",
                    "Files",
                    "Diagnostics",
                    "DiagnosticNext",
                    "DiagnosticPrevious",
                    "Hover",
                    "Definition",
                    "Declaration",
                    "TypeDefinition",
                    "Implementation",
                    "DocumentSymbols",
                    "WorkspaceSymbols",
                    "FormatRange",
                    "References",
                    "Format",
                    "CodeActions",
                    "LspManager",
                    "LspInfo",
                    "LspRestart",
                    "GitStatus",
                    "GitDiff",
                    "GitStage",
                    "GitUnstage",
                ]
                .into_iter()
                .map(|command| super::services::Row {
                    label: command.into(),
                    detail: String::new(),
                    choice: super::services::Choice::Command(command.into()),
                })
                .collect();
                self.services.show(&self.model, "Commands", rows);
            }
            "Implementation" => self
                .services
                .request(&self.model, Kind::Implementation, None),
            "Signature" => self.services.request(&self.model, Kind::Signature, None),
            "LspInfo" => self.services.info(&self.model),
            "Diagnostics" => self.services.diagnostics_at(&self.model),
            "DiagnosticsAll" => self.services.show_diagnostics(&self.model),
            "Files" | "Grep" => {
                self.project_search(arg.to_owned(), name == "Grep", cx);
                return;
            }
            "GitStatus" | "GitDiff" | "GitStage" | "GitUnstage" | "GitCommit" => {
                self.git(name, arg, cx);
                return;
            }
            "bn" | "bnext" => self.model.cycle(false),
            "bp" | "bprevious" => self.model.cycle(true),
            "q" | "quit" if self.model.order.len() == 1 => {
                self.close(window, cx);
                return;
            }
            "q!" if self.model.order.len() == 1 => {
                let id = self.model.id();
                let _ = self.model.close_document(id, true);
                self.finish_quit(cx);
                return;
            }
            "bd" | "q" | "quit" => {
                self.close_document(self.model.id(), window, cx);
                return;
            }
            "bd!" | "q!" => {
                let id = self.model.id();
                let _ = self.model.close_document(id, true);
            }
            "qa" => {
                self.close(window, cx);
                return;
            }
            "Explorer" => self.toggle_explorer(),
            "ExplorerFocus" => {
                if self.explorer.index.is_none() {
                    self.explorer.open(
                        self.explorer.root.clone(),
                        self.model.editor.config.navigation.clone(),
                    );
                }
                self.explorer.focused = true;
                self.explorer.visible = true;
            }
            "SidebarWidth" => {
                if let Ok(width) = arg.parse::<f32>() {
                    self.explorer.width = width.clamp(140., 600.);
                }
            }
            "OpenProject" => {
                self.project_picker(cx);
                return;
            }
            "Hover" => self.services.request(&self.model, Kind::Hover, None),
            "DiagnosticNext" => self.services.navigate_diagnostic(&mut self.model, false),
            "DiagnosticPrevious" => self.services.navigate_diagnostic(&mut self.model, true),
            "DocumentSymbols" => self
                .services
                .request(&self.model, Kind::DocumentSymbols, None),
            "WorkspaceSymbols" => self.services.request(
                &self.model,
                Kind::WorkspaceSymbols,
                Some(serde_json::json!({"query":arg})),
            ),
            "FormatRange" => self
                .services
                .request(&self.model, Kind::RangeFormatting, None),
            "Definition" => self.services.request(&self.model, Kind::Definition, None),
            "Declaration" => self.services.request(&self.model, Kind::Declaration, None),
            "TypeDefinition" => self
                .services
                .request(&self.model, Kind::TypeDefinition, None),
            "References" => self.services.request(&self.model, Kind::References, None),
            "Complete" => self.services.request(&self.model, Kind::Completion, None),
            "Format" => self.services.request(&self.model, Kind::Formatting, None),
            "CodeActions" => self.services.request(&self.model, Kind::Actions, None),
            "Rename" => self.services.request(
                &self.model,
                Kind::Rename,
                Some(serde_json::json!({"newName":arg})),
            ),
            "LspRestart" => self.services.restart(),
            "e!" if arg.is_empty() => {
                if let Some(path) = self.model.editor.buffer.path.clone() {
                    self.reload = Some((self.model.id(), self.model.editor.buffer.revision));
                    self.request(
                        Request::Open {
                            path,
                            revision: self.model.editor.buffer.revision,
                        },
                        cx,
                    );
                }
            }
            "e" | "edit" | "e!" if !arg.is_empty() => {
                self.request(
                    Request::Open {
                        path: PathBuf::from(arg),
                        revision: self.model.editor.buffer.revision,
                    },
                    cx,
                );
            }
            _ => {
                if let Some(request) = self.model.editor.execute_command(&command) {
                    self.request(request, cx);
                }
            }
        }
        self.changed(cx);
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.services.file_busy = self.workspace_file_busy();
        if self.services.workspace_executing
            || self.services.workspace_preparing() && event.keystroke.key == "escape"
        {
            if event.keystroke.key == "escape" {
                self.services.cancel_workspace(&self.model);
                self.changed(cx);
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        #[cfg(feature = "perf")]
        {
            self.input_started = Some(std::time::Instant::now());
        }
        if self.manager_ui.open {
            self.manager_key(event, window, cx);
            return;
        }
        self.typed_character = None;
        let k = &event.keystroke;
        if self.prompt {
            return;
        }
        if let Some(confirm) = &mut self.confirmation {
            let count = match &confirm.decision {
                Decision::Close | Decision::Recovery(_) | Decision::Workspace { .. } => 3,
                Decision::Conflict {
                    overwrite: true, ..
                } => 2,
                _ => 4,
            };
            let cancel = match &confirm.decision {
                Decision::Close | Decision::Recovery(_) | Decision::Workspace { .. } => 2,
                _ => 1,
            };
            if matches!(k.key.as_str(), "space" | "d")
                && let Decision::Workspace { expanded, .. } = &mut confirm.decision
            {
                *expanded = !*expanded;
                cx.notify();
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            if let Decision::Workspace {
                token,
                expanded: true,
                position,
            } = &mut confirm.decision
                && matches!(
                    k.key.as_str(),
                    "up" | "down" | "pageup" | "pagedown" | "home" | "end"
                )
            {
                let count = self
                    .services
                    .workspace_approval
                    .as_ref()
                    .filter(|a| a.token == *token)
                    .map_or(0, |a| a.summaries.len() + a.reasons.len());
                *position = match k.key.as_str() {
                    "home" => 0,
                    "end" => count.saturating_sub(1),
                    "up" => position.saturating_sub(1),
                    "down" => (*position + 1).min(count.saturating_sub(1)),
                    "pageup" => position.saturating_sub(8),
                    _ => (*position + 8).min(count.saturating_sub(1)),
                };
                self.workspace_scroll
                    .scroll_to_item(*position, ScrollStrategy::Top);
                cx.notify();
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            match k.key.as_str() {
                "escape" => self.answer_close(cancel, cx),
                "enter" => {
                    let selected = confirm.selected;
                    self.answer_close(selected, cx);
                }
                "left" | "up" => confirm.selected = (confirm.selected + count - 1) % count,
                "right" | "down" | "tab" => {
                    confirm.selected =
                        (confirm.selected + if k.modifiers.shift { count - 1 } else { 1 }) % count
                }
                _ => {}
            }
            cx.stop_propagation();
            window.prevent_default();
            cx.notify();
            return;
        }
        if k.modifiers.control && self.model.marked.is_none() && matches!(k.key.as_str(), "f" | "h")
        {
            self.dismiss_popup();
            self.model.editor.set_mode(Mode::Command);
            self.model.editor.command = if k.key == "f" { "find " } else { "replace " }.into();
            self.changed(cx);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        if k.key == "f3" {
            self.document_search(k.modifiers.shift, None, cx);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        if self.model.marked.is_some() {
            self.dismiss_popup();
            return;
        }
        if k.key == "tab"
            && self.model.editor.mode == Mode::Insert
            && self.model.editor.snippet_next(k.modifiers.shift)
        {
            self.dismiss_popup();
            self.changed(cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        if let Some(panel) = &mut self.services.panel {
            if panel.presentation == super::popup::Presentation::Completion {
                if k.key == "f1" || self.documentation_only && k.key == "escape" {
                    self.documentation_only = !self.documentation_only;
                    self.popup_scroll.set_offset(point(px(0.), px(0.)));
                    cx.stop_propagation();
                    window.prevent_default();
                    cx.notify();
                    return;
                }
                if self.documentation_only && matches!(k.key.as_str(), "pagedown" | "pageup") {
                    let offset = self.popup_scroll.offset();
                    self.popup_scroll.set_offset(point(
                        offset.x,
                        offset.y + px(if k.key == "pagedown" { -120. } else { 120. }),
                    ));
                    cx.stop_propagation();
                    window.prevent_default();
                    cx.notify();
                    return;
                }
            }
            use super::popup::Route;
            let config = &self.model.editor.config.completion;
            let route = super::popup::route(
                panel.presentation,
                &k.key,
                k.modifiers.control,
                k.modifiers.control || k.modifiers.alt || k.modifiers.platform,
                k.modifiers.shift,
                config.accept_enter,
                config.accept_tab,
            );
            match route {
                Route::Dismiss => self.dismiss_popup(),
                Route::Next => {
                    panel.selected = (panel.selected + 1).min(panel.rows.len().saturating_sub(1))
                }
                Route::Previous => panel.selected = panel.selected.saturating_sub(1),
                Route::Accept => self.accept_popup(window, cx),
                Route::Scroll(direction) => {
                    let offset = self.popup_scroll.offset();
                    self.popup_scroll
                        .set_offset(point(offset.x, offset.y - px(direction as f32 * 120.)));
                }
                Route::Block => {}
                Route::Pass => {}
            }
            if matches!(route, Route::Next | Route::Previous) {
                if let Some(p) = &self.services.panel {
                    self.popup_list_scroll
                        .scroll_to_item(p.selected, ScrollStrategy::Top);
                }
                self.documentation_scroll.set_offset(point(px(0.), px(0.)));
                self.services.selection_changed(&self.model);
            }
            if route != Route::Pass {
                cx.stop_propagation();
                window.prevent_default();
                cx.notify();
                return;
            }
        }
        if self.explorer.focused {
            match k.key.as_str() {
                "escape" => self.explorer.focused = false,
                "up" | "w" => self.explorer.selected = self.explorer.selected.saturating_sub(1),
                "down" | "s" => {
                    self.explorer.selected =
                        (self.explorer.selected + 1).min(self.explorer.rows.len().saturating_sub(1))
                }
                "enter" | "right" => {
                    if let Some(path) = self.explorer.activate(self.explorer.selected) {
                        self.explorer.focused = false;
                        self.request(
                            Request::Open {
                                path,
                                revision: self.model.editor.buffer.revision,
                            },
                            cx,
                        );
                    }
                }
                _ => {}
            }
            self.explorer_scroll
                .scroll_to_item(self.explorer.selected, ScrollStrategy::Top);
            cx.notify();
            window.prevent_default();
            return;
        }
        if k.modifiers.control {
            match k.key.as_str() {
                "e" => {
                    if !self.explorer.visible {
                        self.command("Explorer".into(), window, cx);
                    }
                    self.explorer.focused = !self.explorer.focused;
                    cx.notify();
                    window.prevent_default();
                    return;
                }
                "tab" => {
                    self.model.cycle(k.modifiers.shift);
                    self.changed(cx);
                    window.prevent_default();
                    return;
                }
                "w" => {
                    self.close_document(self.model.id(), window, cx);
                    window.prevent_default();
                    return;
                }
                "space" => {
                    self.services
                        .request(&self.model, super::services::Kind::Completion, None);
                    self.changed(cx);
                    window.prevent_default();
                    return;
                }
                "o" if k.modifiers.shift => {
                    self.project_picker(cx);
                    window.prevent_default();
                    return;
                }
                _ => {}
            }
        }
        if (k.modifiers.control || k.modifiers.platform) && !k.modifiers.alt {
            match k.key.as_str() {
                "o" => {
                    self.open_picker(cx);
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                "s" => {
                    self.save(false, k.modifiers.shift, cx);
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                "c" | "x" if self.model.editor.mode == Mode::Visual => {
                    let action = if k.key == "c" {
                        Action::Copy
                    } else {
                        Action::Cut
                    };
                    if let Some(r) = self.model.act(action) {
                        self.request(r, cx);
                    }
                    self.changed(cx);
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                "z" if k.modifiers.platform => {
                    let action = if k.modifiers.shift {
                        Action::Redo
                    } else {
                        Action::Undo
                    };
                    self.model.act(action);
                    self.changed(cx);
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                "v" if self.input_enabled() || self.model.editor.mode == Mode::Visual => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        if self.model.editor.mode == Mode::Visual {
                            self.model.editor.paste_external(&text);
                        } else {
                            self.model.replace_input(None, &text, false, None);
                        }
                        self.changed(cx);
                    }
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                _ => {}
            }
        }
        let Some(key) = input::translate(k) else {
            return;
        };
        // TSF/IME owns printable Insert/Command text, including AltGr. Modal
        // shortcuts must not also be inserted by the platform's text callback.
        if self.model.marked.is_some() && key.code != KeyCode::Esc {
            return;
        }
        if self.model.editor.mode == Mode::Visual
            && self.model.editor.config.pairs.surround
            && let Some(c) = crate::input::text_character(key)
            && crate::pairs::surround_closer(
                &self.model.editor.buffer,
                c,
                &self.model.editor.config.pairs,
            )
            .is_some()
        {
            self.model.editor.type_character(c);
            self.changed(cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        if self.model.editor.mode == Mode::Insert && self.model.marked.is_none() {
            self.typed_character = crate::input::text_character(key);
        }
        let (handled, request) = self.model.key(key);
        if let Some(command) = self.model.command.take() {
            self.command(command, window, cx);
            window.prevent_default();
            return;
        }
        if let Some(action) = self.model.action.take() {
            use super::services::Kind;
            match action {
                Action::ToggleComment | Action::Unindent => {}
                Action::Buffers => self.command("Buffers".into(), window, cx),
                Action::Palette => self.command("Commands".into(), window, cx),
                Action::FindFiles | Action::LiveGrep | Action::RenameSymbol | Action::Settings => {
                    self.model.editor.set_mode(Mode::Command);
                    self.model.editor.command = match action {
                        Action::FindFiles => "Files ",
                        Action::LiveGrep => "Grep ",
                        Action::RenameSymbol => "Rename ",
                        _ => "set ",
                    }
                    .into();
                }
                Action::Complete => self.services.request(&self.model, Kind::Completion, None),
                Action::Implementation => {
                    self.services
                        .request(&self.model, Kind::Implementation, None)
                }
                Action::CodeActions => self.services.request(&self.model, Kind::Actions, None),
                Action::SignatureHelp => self.services.request(&self.model, Kind::Signature, None),
                Action::LspInfo => self.services.info(&self.model),
                Action::Projects => self.project_picker(cx),
                Action::NextBuffer => self.model.cycle(false),
                Action::PreviousBuffer => self.model.cycle(true),
                Action::CloseBuffer => self.close_document(self.model.id(), window, cx),
                Action::Explorer => self.command("Explorer".into(), window, cx),
                Action::Hover => self.services.request(&self.model, Kind::Hover, None),
                Action::Definition => self.services.request(&self.model, Kind::Definition, None),
                Action::References => self.services.request(&self.model, Kind::References, None),
                Action::FormatDocument => {
                    self.services.request(&self.model, Kind::Formatting, None)
                }
                Action::GitStatus => self.git("GitStatus", "", cx),
                Action::GitDiff => self.git("GitDiff", "", cx),
                Action::GitStage => self.git("GitStage", "", cx),
                Action::GitUnstage => self.git("GitUnstage", "", cx),
                _ => {}
            }
        }

        if let Some(request) = request {
            self.request(request, cx);
        }
        if handled || !self.input_enabled() {
            cx.stop_propagation();
            window.prevent_default();
            self.changed(cx);
        }
    }
    fn mouse(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_popup();
        self.explorer.focused = false;
        self.focus(window, cx);
        if self.prompt || self.manager_ui.open || self.model.editor.mode == Mode::Command {
            return;
        }
        if let Some(at) = self.char_at(event.position) {
            self.model.unmark();
            if event.modifiers.shift {
                if self.model.editor.mode != Mode::Visual {
                    self.model.editor.set_mode(Mode::Visual);
                }
            } else {
                if self.model.editor.mode == Mode::Visual {
                    self.model.editor.set_mode(Mode::Normal);
                }
                self.model.editor.anchor = None;
            }
            self.model.editor.buffer.cursor = at;
            self.model.editor.buffer.snap_cursor();
            self.dragging = true;
            cx.notify();
        }
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging
            && let Some(at) = self.char_at(event.position)
        {
            let old = self.model.editor.buffer.cursor;
            if old != at {
                if self.model.editor.mode != Mode::Visual {
                    self.model.editor.set_mode(Mode::Visual);
                }
                if self.model.editor.anchor.is_none() {
                    self.model.editor.anchor = Some(old);
                }
                self.model.editor.buffer.cursor = at;
                self.model.editor.buffer.snap_cursor();
                cx.notify();
            }
        }
    }
    fn wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_popup();
        let delta = event.delta.pixel_delta(px(self.line_height()));
        let mut x: f32 = delta.x.into();
        let mut y: f32 = delta.y.into();
        if event.modifiers.shift && x == 0. {
            std::mem::swap(&mut x, &mut y);
        }
        self.scroll_y -= y / self.line_height();
        self.scroll_x -= x / f32::from(self.cell).max(1.);
        let dy = self.scroll_y.trunc() as isize;
        let dx = self.scroll_x.trunc() as isize;
        self.scroll_y -= dy as f32;
        self.scroll_x -= dx as f32;
        if dx != 0 || dy != 0 {
            let e = &mut self.model.editor;
            e.top = e
                .top
                .saturating_add_signed(dy)
                .min(e.buffer.text.len_lines().saturating_sub(1));
            e.left = e.left.saturating_add_signed(dx).min(10_000_000);
            self.follow_cursor = false;
            cx.notify();
        }
    }
    fn char_at(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds?;
        if !bounds.contains(&position) {
            return None;
        }
        let row = ((position.y - bounds.top()) / px(self.line_height())).floor() as usize;
        let row = self.cache.as_ref()?.lines.get(row)?;
        let byte = row.shape.closest_index_for_x(position.x - row.origin.x);
        Some(row.source.char_for_byte(byte))
    }
    fn prepare(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let line_height = self.line_height();
        let e = &mut self.model.editor;
        let font = font(FONT_FAMILY);
        let id = window.text_system().resolve_font(&font);
        self.cell = window
            .text_system()
            .advance(id, px(e.config.font_size), 'm')
            .map_or(px(10.), |s| s.width);
        let gutter = if e.config.line_numbers {
            let digits = e.buffer.text.len_lines().max(1).ilog10() + 1;
            GUTTER.max((digits as f32 + 2.0) * f32::from(self.cell))
        } else {
            PADDING
        };
        self.gutter = gutter;
        let rows = (bounds.size.height / px(line_height)).ceil().max(1.) as usize;
        let columns = ((bounds.size.width - px(gutter + PADDING)) / self.cell)
            .floor()
            .max(1.) as usize;
        e.page_height = rows.saturating_sub(1).max(1);
        if self.follow_cursor {
            let row = e.buffer.row();
            if row < e.top {
                e.top = row;
            } else if row >= e.top + e.page_height {
                e.top = row + 1 - e.page_height;
            }
            let column = e.buffer.column(e.config.tab_width);
            if column < e.left {
                e.left = column;
            } else if column >= e.left + columns.saturating_sub(2).max(1) {
                e.left = column.saturating_sub(columns.saturating_sub(3));
            }
            self.follow_cursor = false;
        }
        let valid = self.cache.as_ref().is_some_and(|c| {
            c.document == e.document_id
                && c.epoch == self.services.epoch
                && c.revision == e.buffer.revision
                && c.top == e.top
                && c.left == e.left
                && c.columns == columns
                && c.rows == rows
                && c.search == e.last_search
        });
        if !valid {
            // Keep at most a viewport of shapes; revision/style changes rebuild safely.
            let reusable = self.cache.take().filter(|c| {
                c.document == e.document_id
                    && c.revision == e.buffer.revision
                    && c.epoch == self.services.epoch
                    && c.left == e.left
                    && c.columns == columns
                    && c.search == e.last_search
            });
            let mut previous = reusable.into_iter().flat_map(|c| c.lines).peekable();
            let mut lines = Vec::with_capacity(rows);
            for row in e.top..(e.top + rows).min(e.buffer.text.len_lines()) {
                while previous.peek().is_some_and(|r| r.source.row < row) {
                    previous.next();
                }
                if previous.peek().is_some_and(|r| r.source.row == row)
                    && let Some(reused) = previous.next()
                {
                    lines.push(reused);
                    self.reused_rows += 1;
                    continue;
                }
                self.shaped_rows += 1;
                let source = VisibleLine::new(&e.buffer, row, e.left, columns, e.config.tab_width);
                let byte_range = e.buffer.text.char_to_byte(source.start)
                    ..e.buffer.text.char_to_byte(source.end);
                let spans = self.services.highlights(e, byte_range);
                let search = if e.search_projected_revision == e.buffer.revision {
                    let first = e.search_ranges.partition_point(|r| r.end < source.start);
                    e.search_ranges[first..]
                        .iter()
                        .take_while(|r| r.start <= source.end)
                        .map(|r| {
                            (
                                source.byte_for_char(r.start.max(source.start)),
                                source.byte_for_char(r.end.min(source.end)),
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let runs = syntax_runs(&source, &e.buffer, &spans, &self.theme);
                let shaped = window.text_system().shape_line(
                    source.text.clone().into(),
                    px(e.config.font_size),
                    &runs,
                    None,
                );
                let number = shape(
                    window,
                    &(row + 1).to_string(),
                    e.config.font_size - 2.,
                    self.theme.gutter,
                );
                lines.push(Row {
                    search,
                    source,
                    shape: shaped,
                    number,
                    origin: point(px(0.), px(0.)),
                });
            }
            self.cache = Some(Cache {
                document: e.document_id,
                revision: e.buffer.revision,
                epoch: self.services.epoch,
                top: e.top,
                left: e.left,
                columns,
                rows,
                lines,
                search: e.last_search.clone(),
            });
        }
        if let Some(cache) = &mut self.cache {
            for (i, row) in cache.lines.iter_mut().enumerate() {
                row.origin = point(
                    bounds.left()
                        + px(gutter)
                        + (row.source.column as f32 - e.left as f32) * self.cell,
                    bounds.top() + i as f32 * px(line_height),
                );
            }
        }
        self.bounds = Some(bounds);
        let e = &self.model.editor;
        let color = self.theme.mode(e.mode);
        let row = e.buffer.row();
        if e.config.line_numbers
            && self.active_number.as_ref().is_none_or(|n| {
                n.row != row || n.color != color || n.font_size != e.config.font_size
            })
        {
            self.active_number = Some(ActiveNumber {
                row,
                color,
                font_size: e.config.font_size,
                line: shape_weighted(
                    window,
                    &(row + 1).to_string(),
                    e.config.font_size - 2.,
                    color,
                    FontWeight::BOLD,
                ),
            });
        }
        // The stage settles its mode light toward the current mode.
        if self.stage_mode != Some(color) {
            self.stage_mode = Some(color);
            let stage = self.stage.clone();
            cx.defer(move |cx| stage.update(cx, |stage, cx| stage.set_mode(color, cx)));
        }
        // Command input publishes its destination later in this same prepaint.
        if e.mode == Mode::Command {
            return;
        }
        let target = if !self.explorer.focused
            && !self.manager_ui.open
            && !self.prompt
            && self.confirmation.is_none()
            && self.model.marked.is_none()
            && matches!(e.mode, Mode::Normal | Mode::Insert | Mode::Visual)
        {
            self.caret_bounds().and_then(|caret| {
                let row = self
                    .cache
                    .as_ref()?
                    .lines
                    .iter()
                    .find(|r| r.source.row == e.buffer.row())?;
                let x = row
                    .shape
                    .x_for_index(row.source.byte_for_char(e.buffer.cursor));
                let width = if e.mode == Mode::Insert {
                    px(2.)
                } else {
                    row.source
                        .boundaries
                        .iter()
                        .find(|p| p.0 > e.buffer.cursor)
                        .map_or(self.cell, |p| {
                            (row.shape.x_for_index(p.1) - x).max(self.cell)
                        })
                };
                Some(super::cursor::Geometry {
                    x: f32::from(caret.left()),
                    y: f32::from(caret.top()),
                    width: f32::from(width),
                    height: self.line_height(),
                })
            })
        } else {
            None
        };
        let scene = super::cursor_view::Scene {
            document: e.document_id,
            top: e.top,
            left: e.left,
            bounds,
            mode: e.mode,
            scale: window.scale_factor(),
            font_size: e.config.font_size,
            cell_width: f32::from(self.cell),
        };
        let clip = Bounds::new(
            point(bounds.left() + px(gutter), bounds.top()),
            size(
                (bounds.size.width - px(gutter)).max(px(0.)),
                bounds.size.height,
            ),
        );
        // The caret carries the mode color: the mode is legible where you look.
        let caret = self.theme.mode(e.mode);
        self.cursor.update(cx, |cursor, cx| {
            cursor.update_target(target, scene, &e.config.cursor, caret, clip, cx)
        });
    }
    fn prepare_command(
        &mut self,
        bounds: Bounds<Pixels>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let e = &self.model.editor;
        // Typed input is primary content; feedback elsewhere stays secondary.
        let color = if e.error {
            self.theme.error
        } else {
            self.theme.text
        };
        if self.command_line.as_ref().is_none_or(|c| {
            c.line.text.as_ref() != text.as_str()
                || c.font_size != e.config.font_size
                || c.color != color
        }) {
            self.command_line = Some(CommandLine {
                line: shape(window, &text, e.config.font_size, color),
                font_size: e.config.font_size,
                color,
                origin: bounds.origin,
                bounds,
            });
        }
        let Some(line) = &mut self.command_line else {
            return;
        };
        let overflow = (line.line.width - (bounds.size.width - px(2.)).max(px(0.))).max(px(0.));
        line.origin = point(bounds.left() - overflow, bounds.top());
        line.bounds = bounds;
        let target = if !self.prompt && self.confirmation.is_none() && !self.explorer.focused {
            Some(super::cursor::Geometry {
                x: f32::from(line.origin.x + line.line.width),
                y: f32::from(bounds.top()),
                width: 2.,
                height: self.line_height(),
            })
        } else {
            None
        };
        let Some(editor_bounds) = self.bounds else {
            return;
        };
        let scene = super::cursor_view::Scene {
            document: e.document_id,
            top: e.top,
            left: e.left,
            bounds: editor_bounds,
            mode: e.mode,
            scale: window.scale_factor(),
            font_size: e.config.font_size,
            cell_width: f32::from(self.cell),
        };
        let caret = self.theme.mode(e.mode);
        self.cursor.update(cx, |cursor, cx| {
            cursor.update_target(target, scene, &e.config.cursor, caret, bounds, cx)
        });
    }
    fn paint_command(&self, window: &mut Window, cx: &mut App) {
        if let Some(line) = &self.command_line {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: line.bounds,
                }),
                |window| {
                    if let Err(error) =
                        line.line
                            .paint(line.origin, px(self.line_height()), window, cx)
                    {
                        eprintln!("Command paint: {error}");
                    }
                },
            );
        }
    }
    fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let e = &self.model.editor;
        let bounds = self.bounds?;
        let row = self.cache.as_ref()?.lines.iter().find(|r| {
            r.source.row == e.buffer.row()
                && e.buffer.cursor >= r.source.start
                && e.buffer.cursor <= r.source.end
        })?;
        let origin = point(
            row.origin.x
                + row
                    .shape
                    .x_for_index(row.source.byte_for_char(e.buffer.cursor)),
            row.origin.y,
        );
        let caret = Bounds::new(origin, size(self.cell, px(self.line_height())));
        (caret.left() >= bounds.left() + px(self.gutter)
            && caret.left() < bounds.right()
            && caret.top() < bounds.bottom())
        .then_some(caret)
    }
    fn popup_row(
        &self,
        i: usize,
        row_height: f32,
        request: u64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use super::services::Choice;
        let panel = self.services.panel.as_ref()?;
        if panel.request != request {
            return None;
        }
        let row = panel.rows.get(i)?;
        let theme = self.theme;
        let scale = self.scale();
        let size = if panel.presentation == super::popup::Presentation::Completion {
            self.model.editor.config.font_size.min(20.)
        } else {
            scale.ui
        };
        let file_glyph = |name: &str| {
            let (glyph, hue) = icons::file(name);
            (glyph, hue.map_or(theme.secondary, |h| theme.hue(h)))
        };
        let mut meta = None;
        let mut highlights = Vec::new();
        let (glyph, color, label) = match &row.choice {
            Choice::Completion(value) => {
                let end = row.label.rfind("  · ").unwrap_or(row.label.len());
                let kind = value["kind"].as_u64().unwrap_or(0);
                highlights.extend(
                    super::completion::matching_ranges(
                        &row.label[..end],
                        self.services.completion_query(),
                    )
                    .into_iter()
                    .map(|range| {
                        (
                            range,
                            HighlightStyle {
                                color: Some(theme.matched),
                                ..Default::default()
                            },
                        )
                    }),
                );
                meta = row.label.get(end + "  · ".len()..).map(str::to_owned);
                (
                    icons::completion_kind(kind),
                    theme.completion_kind(kind as usize),
                    row.label[..end].to_owned(),
                )
            }
            Choice::Location { path, .. } => {
                let (glyph, color) = file_glyph(
                    &path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                );
                (glyph, color, row.label.clone())
            }
            Choice::Buffer(_) => {
                let name = row.label.trim_end_matches(" [+]");
                let name = name.rsplit(std::path::is_separator).next().unwrap_or(name);
                let (glyph, color) = file_glyph(name);
                (glyph, color, row.label.clone())
            }
            Choice::Command(_) => (icons::CHEVRON_RIGHT, theme.muted, row.label.clone()),
            _ => ("", theme.secondary, row.label.clone()),
        };
        let label = StyledText::new(label).with_highlights(highlights);
        Some(
            design::row(&theme, ("popup-row", i), i == panel.selected)
                .h(px(row_height))
                .child(design::icon(glyph, size, color))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label),
                )
                .when_some(meta, |d, meta| {
                    d.child(
                        div()
                            .flex_none()
                            .pl(px(space::MD))
                            .text_size(px(scale.small))
                            .text_color(theme.muted)
                            .child(meta),
                    )
                })
                .on_mouse_move(cx.listener(move |v, _, _, cx| {
                    if v.services
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.request == request && p.selected != i)
                    {
                        if let Some(p) = &mut v.services.panel {
                            p.selected = i;
                        }
                        v.documentation_scroll.set_offset(point(px(0.), px(0.)));
                        v.services.selection_changed(&v.model);
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, _, w, cx| {
                        cx.stop_propagation();
                        if v.services
                            .panel
                            .as_ref()
                            .is_none_or(|p| p.request != request)
                        {
                            return;
                        }
                        if let Some(p) = &mut v.services.panel {
                            p.selected = i;
                        }
                        v.services.selection_changed(&v.model);
                        v.accept_popup(w, cx);
                    }),
                )
                .into_any_element(),
        )
    }
    /// Modal decision dialog over a scrim. Choices keep their order and indices;
    /// tone marks the safe default and destructive choices, focus follows keys.
    fn confirmation_dialog(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let Some(confirm) = &self.confirmation else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let scale = self.scale();
        let selected = confirm.selected;
        let name = self
            .model
            .get(confirm.document)
            .and_then(|e| e.buffer.path.as_ref())
            .map_or_else(
                || "Untitled".into(),
                |p| design::display_path(p, &self.explorer.root),
            );
        type Choice = (&'static str, Tone);
        let (glyph, color, title, detail, choices): (_, _, &str, &str, &[Choice]) = match &confirm
            .decision
        {
            Decision::Workspace { .. } => (
                icons::EDIT,
                theme.accent,
                "Apply workspace edit?",
                "Approval can change files on disk. Text edits stay unsaved and undoable. Reject or Escape leaves both unchanged.",
                &[
                    ("Approve", Tone::Primary),
                    ("Reject", Tone::Secondary),
                    ("Cancel", Tone::Secondary),
                ],
            ),
            Decision::Recovery(_) => (
                icons::HISTORY,
                theme.info,
                "Recover unsaved documents?",
                "Recovery opens local snapshots without overwriting disk. Later keeps the stored copies.",
                &[
                    ("Recover", Tone::Primary),
                    ("Discard", Tone::Danger),
                    ("Later", Tone::Secondary),
                ],
            ),
            Decision::Close => (
                icons::WARNING,
                theme.warning,
                "Save changes before closing?",
                "Discard loses unsaved changes. Escape cancels.",
                &[
                    ("Save", Tone::Primary),
                    ("Discard", Tone::Danger),
                    ("Cancel", Tone::Secondary),
                ],
            ),
            Decision::Conflict {
                overwrite: false, ..
            } => (
                icons::WARNING,
                theme.warning,
                "File changed outside FilaVim",
                "Reload discards editor changes. Keep preserves both versions. Saving a copy leaves the original intact.",
                &[
                    ("Reload", Tone::Danger),
                    ("Keep", Tone::Primary),
                    ("Save copy", Tone::Secondary),
                    ("Overwrite…", Tone::Danger),
                ],
            ),
            Decision::Conflict {
                overwrite: true, ..
            } => (
                icons::ERROR,
                theme.error,
                "Overwrite external changes?",
                "This replaces the disk version with your editor contents. Further disk changes will stop the save.",
                &[("Overwrite", Tone::Danger), ("Cancel", Tone::Secondary)],
            ),
        };
        let subject = match &confirm.decision {
            Decision::Conflict { path, actual, .. } => {
                let disk = match actual {
                    crate::files::DiskState::Missing => "disk file is missing".into(),
                    crate::files::DiskState::Present { bytes, .. } => {
                        format!("disk {bytes} bytes")
                    }
                };
                let editor = self
                    .model
                    .get(confirm.document)
                    .map_or(0, |e| e.buffer.text.len_bytes());
                format!(
                    "{} · {disk} · editor {editor} bytes",
                    design::display_path(path, &self.explorer.root)
                )
            }
            Decision::Recovery(entries) => format!(
                "{} {}: {}",
                entries.len(),
                if entries.len() == 1 {
                    "document"
                } else {
                    "documents"
                },
                entries
                    .iter()
                    .take(4)
                    .map(|e| e.path.as_ref().map_or_else(
                        || "Untitled".into(),
                        |p| design::display_path(p, &self.explorer.root)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Decision::Workspace { token, .. } => self
                .services
                .workspace_approval
                .as_ref()
                .filter(|a| a.token == *token)
                .map_or_else(
                    || "Expired workspace plan".into(),
                    |a| {
                        format!(
                            "{} ordered changes · {} approval reasons",
                            a.summaries.len(),
                            a.reasons.len()
                        )
                    },
                ),
            Decision::Close => name,
        };
        let subject_glyph = match &confirm.decision {
            Decision::Close | Decision::Conflict { .. } => {
                icons::file(
                    subject
                        .split(" · ")
                        .next()
                        .and_then(|p| p.rsplit(std::path::is_separator).next())
                        .unwrap_or(""),
                )
                .0
            }
            Decision::Recovery(_) => icons::HISTORY,
            Decision::Workspace { .. } => icons::FILE,
        };
        let expanded = matches!(confirm.decision, Decision::Workspace { expanded: true, .. });
        let workspace_details: Option<AnyElement> = if let Decision::Workspace {
            token,
            position,
            ..
        } = confirm.decision
        {
            self.services
                .workspace_approval
                .as_ref()
                .filter(|a| a.token == token)
                .map(|a| {
                    let entries: std::sync::Arc<Vec<String>> = std::sync::Arc::new(
                        a.reasons
                            .iter()
                            .map(|s| format!("Approval: {s}"))
                            .chain(a.summaries.iter().cloned())
                            .collect(),
                    );
                    let well = div()
                        .p(px(space::SM))
                        .rounded(px(design::radius::CONTROL))
                        .border_1()
                        .border_color(theme.hairline)
                        .bg(theme.hover)
                        .text_size(px(scale.small));
                    if expanded {
                        let selected = entries.get(position).cloned().unwrap_or_default();
                        let row_height = scale.row();
                        let list = uniform_list(
                            "workspace-operations",
                            entries.len(),
                            move |range, _, _| {
                                range
                                    .map(|i| {
                                        div()
                                            .h(px(row_height))
                                            .px(px(space::SM))
                                            .flex()
                                            .items_center()
                                            .rounded(px(design::radius::CONTROL))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .when(i == position, |d| {
                                                d.bg(theme.popup_selection).text_color(theme.text)
                                            })
                                            .child(entries[i].clone())
                                            .into_any_element()
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(self.workspace_scroll.clone())
                        .flex_1()
                        .min_h(px(0.));
                        well.flex()
                            .flex_col()
                            .flex_1()
                            .min_h(px(0.))
                            .gap(px(space::SM))
                            .text_color(theme.secondary)
                            .child(list)
                            .child(design::divider(&theme))
                            .child(
                                div()
                                    .id("workspace-operation-details")
                                    .max_h(px(88.))
                                    .overflow_y_scroll()
                                    .text_color(theme.text)
                                    .child(selected),
                            )
                            .into_any_element()
                    } else {
                        well.text_color(theme.secondary)
                            .child(
                                a.reasons
                                    .iter()
                                    .take(3)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            )
                            .into_any_element()
                    }
                })
        } else {
            None
        };
        let buttons = choices
            .iter()
            .copied()
            .enumerate()
            .map(|(i, (label, tone))| {
                design::button(
                    &theme,
                    scale,
                    ("confirm-choice", i),
                    label,
                    tone,
                    true,
                    i == selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        v.answer_close(i, cx);
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let mut keys: Vec<(&'static str, &'static str)> =
            vec![("← →", "choose"), ("Enter", "confirm"), ("Esc", "cancel")];
        if matches!(confirm.decision, Decision::Workspace { .. }) {
            keys.push((
                "D",
                if expanded {
                    "hide changes"
                } else {
                    "show changes"
                },
            ));
        }
        let width = (f32::from(viewport.width) - space::XXL * 2.).clamp(1., 600.);
        let height = (f32::from(viewport.height) - space::XXL * 2.).max(1.);
        let dialog = design::glass(&theme, design::Elevation::Modal)
            .id("confirmation")
            .w(px(width))
            .max_h(px(height))
            .when(expanded, |d| d.h(px(height.min(480.))))
            .overflow_hidden()
            .p(px(space::XL))
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .text_size(px(scale.ui))
            .child(design::heading(&theme, scale, glyph, color, title).text_size(px(scale.ui + 1.)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .text_size(px(scale.small))
                    .text_color(theme.secondary)
                    .child(design::icon(subject_glyph, scale.small, theme.secondary))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(subject),
                    ),
            )
            .child(div().text_color(theme.secondary).child(detail))
            .children(workspace_details)
            .child(design::divider(&theme))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(space::LG))
                    .child(design::hints(&theme, scale, &keys))
                    .child(div().flex().gap(px(space::SM)).children(buttons)),
            );
        design::typeface(design::scrim(&theme, viewport), &theme, scale.ui)
            .id("confirmation-scrim")
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialog)
            .into_any_element()
    }
    fn popups(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, Point<Pixels>)> {
        if self.manager_ui.open {
            return self.manager_popup(window, cx);
        }
        use super::popup::{Presentation, Rect, place};
        let viewport = window.viewport_size();
        let full = Rect {
            x: 0.,
            y: 0.,
            width: f32::from(viewport.width),
            height: f32::from(viewport.height),
        };
        self.popup_bounds.clear();
        if self.confirmation.is_some() {
            let dialog = self.confirmation_dialog(viewport, cx);
            self.popup_frames += 1;
            return vec![(dialog, point(px(0.), px(0.)))];
        }
        let popup_timer = crate::perf::Timer::start();
        let Some(panel) = &self.services.panel else {
            self.popup_request = None;
            #[cfg(feature = "perf")]
            if self.popup_is_completion {
                crate::perf::count("completion.popup_unmount", 1);
                self.popup_is_completion = false;
            }
            return Vec::new();
        };
        if self.popup_request != Some(panel.request) {
            self.popup_request = Some(panel.request);
            #[cfg(feature = "perf")]
            {
                if self.popup_is_completion {
                    crate::perf::count("completion.popup_unmount", 1);
                }
                self.popup_is_completion = panel.kind == super::services::Kind::Completion;
                if self.popup_is_completion {
                    crate::perf::count("completion.popup_mount", 1);
                }
            }
            self.documentation_only = false;
            self.popup_scroll.set_offset(point(px(0.), px(0.)));
            self.popup_list_scroll
                .scroll_to_item_strict(0, ScrollStrategy::Top);
            self.documentation_scroll.set_offset(point(px(0.), px(0.)));
        }
        let theme = self.theme;
        let scale = self.scale();
        let presentation = panel.presentation;
        let selected = panel.selected;
        let contextual = !matches!(
            presentation,
            Presentation::Picker | Presentation::InformationCentered
        );
        let caret = if contextual {
            let Some(bounds) = self.caret_bounds() else {
                self.services.dismiss();
                return Vec::new();
            };
            Some(Rect {
                x: f32::from(bounds.left()),
                y: f32::from(bounds.top()),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            })
        } else {
            None
        };
        let bounds = if contextual {
            self.bounds
                .map(|b| Rect {
                    x: f32::from(b.left()),
                    y: f32::from(b.top()),
                    width: f32::from(b.size.width),
                    height: f32::from(b.size.height),
                })
                .unwrap_or(full)
        } else {
            full
        };
        let information = matches!(
            presentation,
            Presentation::Information | Presentation::InformationCentered
        ) || self.documentation_only;
        let minimal = contextual || self.documentation_only;
        let completion = presentation == Presentation::Completion;
        // Code-bearing surfaces use the code size; navigational ones the UI size.
        let code_size = self.model.editor.config.font_size.min(20.);
        let font_size = if completion || presentation == Presentation::Information {
            code_size
        } else {
            scale.ui
        };
        let row_height = if completion {
            (code_size * 1.5).ceil() + 4.
        } else {
            scale.row()
        };
        let header = scale.row() + space::MD;
        let footer = scale.row();
        let diagnostics = panel.title == "Diagnostics";
        let detail = panel
            .rows
            .get(selected)
            .map(|r| r.detail.clone())
            .filter(|s| !s.is_empty());
        // Minimal floats fit their text: Agave is monospaced, so wrapping can be
        // estimated from the shaped cell advance without a layout pass.
        let advance = f32::from(self.cell) * font_size / self.model.editor.config.font_size;
        let text_height = |texts: &mut dyn Iterator<Item = &str>, width: f32| {
            let columns = ((width - space::LG * 2. - 2.) / advance.max(1.)).max(1.) as usize;
            let line = (font_size * 1.62).ceil();
            texts
                .map(|text| {
                    text.lines()
                        .map(|l| l.chars().count().div_ceil(columns).max(1))
                        .sum::<usize>()
                        .max(1) as f32
                        * line
                        + space::SM
                })
                .sum::<f32>()
                // Headroom for paragraph spacing and rules; floats below the caret
                // shrink-wrap, so overestimating never leaves empty glass there.
                * 1.2
        };
        let mut placement = place(
            bounds,
            caret,
            (
                if contextual { 460. } else { 680. },
                if information {
                    if panel.kind == super::services::Kind::Signature {
                        panel
                            .rows
                            .iter()
                            .map(|r| r.label.lines().count())
                            .sum::<usize>()
                            .clamp(1, 6) as f32
                            * font_size
                            * 1.5
                            + 36.
                    } else if self.documentation_only {
                        (text_height(
                            &mut std::iter::once(detail.as_deref().unwrap_or("Loading")),
                            460.,
                        ) + space::LG
                            + space::MD * 3.
                            + 2.)
                            .min(320.)
                    } else if minimal {
                        (text_height(
                            &mut panel.rows.iter().take(100).map(|r| r.label.as_str()),
                            460.,
                        ) + space::LG
                            + space::MD * 3.
                            + 2.)
                            .min(320.)
                    } else {
                        (text_height(
                            &mut panel.rows.iter().take(100).map(|r| r.label.as_str()),
                            680.,
                        ) + header
                            + footer
                            + space::MD * 2.
                            + 4.)
                            .clamp(header + footer + 72., 440.)
                    }
                } else if completion {
                    panel.rows.len().clamp(1, 8) as f32 * row_height + space::LG + space::MD + 2.
                } else {
                    panel.rows.len().clamp(1, 10) as f32 * row_height
                        + header
                        + footer
                        + space::XS * 2.
                        + 4.
                },
            ),
            detail.is_some() && !self.documentation_only,
        );
        // Pickers hang from the command lens: the query and its results read as
        // one instrument at the top of the editor pane. Without a lens (the
        // palette), they drop from the place the lens would occupy.
        if presentation == Presentation::Picker
            && let Some(frame) = self.lens_frame
        {
            let top = if self.model.editor.mode == Mode::Command {
                f32::from(frame.bottom()) + space::SM
            } else {
                f32::from(frame.top())
            };
            let room = f32::from(viewport.height) - top - space::XXL;
            placement.menu = Rect {
                x: f32::from(frame.left()),
                y: top,
                width: f32::from(frame.size.width),
                height: placement.menu.height.min(room.max(120.)),
            };
            placement.above = false;
        }
        let rows = if self.documentation_only {
            vec![
                div()
                    .child(super::floating::documentation(
                        detail.clone().unwrap_or_else(|| {
                            if self.services.resolving_selected() {
                                "Loading documentation…".into()
                            } else {
                                "No documentation supplied".into()
                            }
                        }),
                        theme,
                    ))
                    .into_any_element(),
            ]
        } else if information && panel.rows.is_empty() {
            vec![
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .text_color(theme.muted)
                    .child(design::icon(icons::INFO, font_size, theme.muted))
                    .child("Nothing to show yet.")
                    .into_any_element(),
            ]
        } else if information {
            panel
                .rows
                .iter()
                .take(100)
                .map(|r| {
                    let severity = diagnostics
                        .then(|| {
                            [
                                ("[Error]", 1),
                                ("[Warning]", 2),
                                ("[Information]", 3),
                                ("[Hint]", 4),
                            ]
                            .into_iter()
                            .find(|(tag, _)| r.label.starts_with(tag))
                        })
                        .flatten();
                    match (severity, &r.choice) {
                        (Some((tag, level)), _) => {
                            // Services format: "[Severity] message\nserver · line:col".
                            let body = r.label[tag.len()..].trim_start();
                            let (message, meta) = body
                                .rsplit_once('\n')
                                .map_or((body, None), |(m, meta)| (m, Some(meta)));
                            // Server keys carry their root after '@'; the panel
                            // already belongs to this document, so show the name.
                            let meta = meta.map(|meta| match meta.split_once(" · ") {
                                Some((server, at)) => format!(
                                    "{} · {at}",
                                    server.split_once('@').map_or(server, |(s, _)| s)
                                ),
                                None => meta.to_owned(),
                            });
                            let (glyph, color) = design::severity(&theme, level);
                            div()
                                .flex()
                                .items_start()
                                .gap(px(space::SM))
                                .mb(px(space::MD))
                                .child(design::icon(glyph, font_size, color))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(message.trim_end().to_owned())
                                        .when_some(meta, |d, meta| {
                                            d.child(
                                                div()
                                                    .text_size(px(scale.small))
                                                    .text_color(theme.muted)
                                                    .child(meta),
                                            )
                                        }),
                                )
                                .into_any_element()
                        }
                        (None, super::services::Choice::Signature(ranges))
                            if r.label.contains('(') =>
                        {
                            div()
                                .mb(px(space::SM))
                                .child(super::floating::signature(&r.label, ranges, theme))
                                .into_any_element()
                        }
                        (None, _) => div()
                            .mb(px(space::SM))
                            .child(super::floating::documentation(r.label.clone(), theme))
                            .into_any_element(),
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let area = placement.menu;
        let origin = point(px(area.x), px(area.y));
        let dimensions = size(px(area.width), px(area.height));
        self.popup_bounds.push(Bounds::new(origin, dimensions));
        let body = if information {
            div()
                .id("popup-content")
                // Auto basis: grows in fixed frames, shrink-wraps in fitted ones.
                .flex_auto()
                .min_h(px(0.))
                .overflow_y_scroll()
                .track_scroll(&self.popup_scroll)
                .px(px(space::LG))
                .py(px(space::MD))
                .children(rows)
                .into_any_element()
        } else {
            let entity = cx.entity();
            let request = panel.request;
            let count = panel.rows.len();
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .when(!minimal, |d| d.py(px(space::XS)))
                .child(
                    uniform_list("popup-list", count, move |range, _, cx| {
                        entity.update(cx, |v, cx| {
                            range
                                .filter_map(|i| v.popup_row(i, row_height, request, cx))
                                .collect()
                        })
                    })
                    .track_scroll(self.popup_list_scroll.clone())
                    .px(px(space::XS))
                    .flex_1()
                    .min_h(px(0.)),
                )
                .into_any_element()
        };
        let (signal, signal_color) = if completion && !self.documentation_only {
            (icons::ZAP, theme.warning)
        } else if diagnostics {
            (icons::WARNING, theme.warning)
        } else if panel.kind == super::services::Kind::Signature {
            (icons::SIGNATURE, theme.matched)
        } else {
            (icons::INFO, theme.matched)
        };
        let title_glyph = match panel.title.as_str() {
            "Commands" => icons::TERMINAL,
            "Files" | "Project grep" => icons::SEARCH,
            "Buffers" => icons::FILE,
            "Diagnostics" => icons::WARNING,
            _ => signal,
        };
        let title = if self.documentation_only {
            "Documentation".to_owned()
        } else {
            panel.title.clone()
        };
        let count = (!information).then(|| format!("{} / {}", selected + 1, panel.rows.len()));
        let fit = information && minimal && !placement.above;
        let surface = super::floating::frame(theme, "floating-popup", font_size)
            .w(dimensions.width)
            .map(|d| {
                if fit {
                    d.max_h(dimensions.height)
                } else {
                    d.h(dimensions.height)
                }
            })
            .when(minimal, |d| {
                d.pt(px(space::LG)).pb(px(space::MD)).child(design::notch(
                    &theme,
                    signal,
                    signal_color,
                    scale.small,
                ))
            })
            .when(!minimal, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(space::MD))
                        .h(px(header))
                        .px(px(space::LG))
                        .child(
                            design::heading(&theme, scale, title_glyph, theme.accent, title)
                                .flex_1()
                                .min_w(px(0.)),
                        )
                        .when_some(count, |d, count| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(scale.micro))
                                    .text_color(theme.muted)
                                    .child(count),
                            )
                        }),
                )
                .child(design::divider(&theme))
            })
            .child(body)
            .when(!minimal, |d| {
                let keys: &[(&'static str, &'static str)] = if information {
                    &[("PgUp PgDn", "scroll"), ("Esc", "close")]
                } else {
                    &[("↑ ↓", "select"), ("Enter", "accept"), ("Esc", "close")]
                };
                d.child(design::divider(&theme)).child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(px(footer))
                        .px(px(space::LG))
                        .overflow_hidden()
                        .child(design::hints(&theme, scale, keys)),
                )
            })
            .into_any_element();
        let mut draws = Vec::with_capacity(4);
        if !contextual {
            // Centered panels take keyboard focus; dim the editor beneath them.
            // The scrim has no listeners, so outside clicks still dismiss.
            draws.push((
                design::scrim(&theme, viewport)
                    .bg(theme.scrim.opacity(0.6))
                    .into_any_element(),
                point(px(0.), px(0.)),
            ));
        }
        draws.push((surface, origin));
        if let (Some(detail), Some(area)) = (detail, placement.documentation) {
            let origin = point(px(area.x), px(area.y));
            let height = (text_height(&mut std::iter::once(detail.as_str()), area.width)
                + (space::MD + space::XS) * 2.
                + 2.)
                .min(area.height);
            let dimensions = size(px(area.width), px(height));
            self.popup_bounds.push(Bounds::new(origin, dimensions));
            draws.push((
                super::floating::frame(theme, "completion-documentation", font_size)
                    .w(dimensions.width)
                    .max_h(dimensions.height)
                    .child(design::notch(
                        &theme,
                        icons::INFO,
                        theme.matched,
                        scale.small,
                    ))
                    .child(
                        div()
                            .id("completion-documentation-content")
                            .flex_auto()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.documentation_scroll)
                            .px(px(space::LG))
                            .py(px(space::MD + space::XS))
                            .child(super::floating::documentation(detail, theme)),
                    )
                    .into_any_element(),
                origin,
            ));
        }
        if let (Some(signature), Some(caret)) = (&self.services.signature, caret) {
            let occupied: Vec<_> = std::iter::once(placement.menu)
                .chain(placement.documentation)
                .collect();
            let lines = signature
                .rows
                .iter()
                .map(|r| r.label.lines().count())
                .sum::<usize>()
                .clamp(1, 4);
            if let Some(area) = super::popup::assistance(
                bounds,
                caret,
                &occupied,
                lines as f32 * code_size * 1.5 + 24.,
            ) {
                let origin = point(px(area.x), px(area.y));
                let dimensions = size(px(area.width), px(area.height));
                self.popup_bounds.push(Bounds::new(origin, dimensions));
                let rows = signature.rows.iter().take(2).map(|row| {
                    let text = match &row.choice {
                        super::services::Choice::Signature(active) => {
                            super::floating::signature(&row.label, active, theme)
                        }
                        _ => StyledText::new(row.label.clone()),
                    };
                    div().child(text)
                });
                draws.push((
                    super::floating::frame(theme, "signature-assistance", code_size)
                        .w(dimensions.width)
                        .h(dimensions.height)
                        .child(design::notch(
                            &theme,
                            icons::SIGNATURE,
                            theme.matched,
                            scale.small,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.))
                                .overflow_hidden()
                                .px(px(space::LG))
                                .py(px(space::MD + space::XS))
                                .children(rows),
                        )
                        .into_any_element(),
                    origin,
                ));
            }
        }
        self.popup_frames += 1;
        popup_timer.finish("completion.popup_build");
        draws
    }
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.painted_frames += 1;
        let e = &self.model.editor;
        let Some(cache) = &self.cache else {
            return;
        };
        let gutter = self.gutter;
        let mask = ContentMask {
            bounds: Bounds::new(
                point(bounds.left() + px(gutter), bounds.top()),
                size(
                    (bounds.size.width - px(gutter)).max(px(0.)),
                    bounds.size.height,
                ),
            ),
        };
        let selection = self.model.selection();
        let guides = self.indent_guides(cache);
        let text_left = bounds.left() + px(gutter) - self.cell * e.left as f32;
        window.with_content_mask(Some(mask), |window| {
            // Indent guides: hairlines at each indentation level; the guide that
            // encloses the cursor's block is lit a little brighter.
            for (row, (levels, active)) in cache.lines.iter().zip(&guides) {
                for level in 0..*levels {
                    let column = level * guides.unit;
                    if column < e.left {
                        continue;
                    }
                    let x = text_left + self.cell * column as f32;
                    window.paint_quad(fill(
                        Bounds::new(point(x, row.origin.y), size(px(1.), px(self.line_height()))),
                        if Some(level) == *active {
                            self.theme.text.opacity(0.16)
                        } else {
                            self.theme.hairline
                        },
                    ));
                }
            }
            for row in &cache.lines {
                if e.buffer.row() == row.source.row {
                    // The current line is a faint band of light across the text
                    // column, rounded so it reads as glass rather than a stripe.
                    let left = bounds.left() + px(gutter - PADDING * 0.5);
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(left, row.origin.y),
                                size(
                                    (bounds.right() - left - px(space::LG)).max(px(0.)),
                                    px(self.line_height()),
                                ),
                            ),
                            self.theme.active_line,
                        )
                        .corner_radii(Corners::all(px(design::radius::CONTROL))),
                    );
                }
                // Matches keep their syntax colors (no reshaping on navigation):
                // every match is a quiet tint, the current one is filled and outlined.
                for &(start, end) in &row.search {
                    let left = row.shape.x_for_index(start);
                    let right = row.shape.x_for_index(end);
                    let current = e.search_revision == e.buffer.revision
                        && e.search_ranges
                            .binary_search_by_key(&e.buffer.cursor, |r| r.start)
                            .ok()
                            .is_some_and(|i| {
                                let r = &e.search_ranges[i];
                                r.start == e.buffer.cursor
                                    && r.start >= row.source.start
                                    && r.start <= row.source.end
                                    && row.source.byte_for_char(r.start) == start
                            });
                    let color = self.theme.search;
                    window.paint_quad(quad(
                        Bounds::new(
                            point(row.origin.x + left, row.origin.y + px(2.)),
                            size((right - left).max(px(1.)), px(self.line_height() - 4.)),
                        ),
                        Corners::all(px(design::radius::CONTROL - 1.)),
                        color.opacity(if current { 0.32 } else { 0.16 }),
                        px(if current { 1. } else { 0. }),
                        color.opacity(0.9),
                        BorderStyle::Solid,
                    ));
                }
                if !selection.is_empty()
                    && selection.end > row.source.start
                    && selection.start <= row.source.end
                {
                    let start = row.shape.x_for_index(
                        row.source
                            .byte_for_char(selection.start.max(row.source.start)),
                    );
                    let end = if selection.end > row.source.end {
                        row.shape.width + self.cell
                    } else {
                        row.shape
                            .x_for_index(row.source.byte_for_char(selection.end))
                    };
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(row.origin.x + start, row.origin.y),
                                size((end - start).max(px(1.)), px(self.line_height())),
                            ),
                            self.theme.selection,
                        )
                        .corner_radii(Corners::all(px(design::radius::INNER - 1.))),
                    );
                }
                if let Some(marked) = &self.model.marked
                    && marked.end > row.source.start
                    && marked.start <= row.source.end
                {
                    let start = row
                        .shape
                        .x_for_index(row.source.byte_for_char(marked.start));
                    let end = row.shape.x_for_index(row.source.byte_for_char(marked.end));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                row.origin.x + start,
                                row.origin.y + px(self.line_height() - 2.),
                            ),
                            size((end - start).max(px(1.)), px(1.)),
                        ),
                        self.theme.accent,
                    ));
                }
                if let Err(error) = row
                    .shape
                    .paint(row.origin, px(self.line_height()), window, cx)
                {
                    eprintln!("Text paint: {error}");
                }
            }
        });
        self.paint_rail(bounds, window);
        if e.config.line_numbers {
            for row in &cache.lines {
                let diagnostic = self
                    .services
                    .diagnostics
                    .iter()
                    .filter(|((_, id), (revision, _, _))| {
                        *id == e.document_id && *revision == e.buffer.revision
                    })
                    .find_map(|(_, (_, _, items))| {
                        let i =
                            items.partition_point(|d| d.range.start.line < (row.source.row as u32));
                        items
                            .get(i)
                            .filter(|d| d.range.start.line == row.source.row as u32)
                    });
                if let Some(diagnostic) = diagnostic {
                    // A round severity mark in the gutter's leading margin.
                    let mark = px(6.);
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(
                                    bounds.left() + px(8.),
                                    row.origin.y + (px(self.line_height()) - mark) / 2.,
                                ),
                                size(mark, mark),
                            ),
                            self.theme.diagnostic(diagnostic.severity),
                        )
                        .corner_radii(Corners::all(mark / 2.)),
                    );
                }
                let number = self
                    .active_number
                    .as_ref()
                    .filter(|n| n.row == row.source.row && e.buffer.row() == n.row)
                    .map_or(&row.number, |n| &n.line);
                let origin = point(
                    bounds.left() + px(gutter - 14.) - number.width,
                    row.origin.y,
                );
                if let Err(error) = number.paint(origin, px(self.line_height()), window, cx) {
                    eprintln!("Gutter paint: {error}");
                }
            }
        }
    }
}
fn syntax_runs(
    source: &VisibleLine,
    buffer: &crate::buffer::Buffer,
    spans: &[crate::syntax::Highlight],
    theme: &Theme,
) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    for pair in source.boundaries.windows(2) {
        let (at, start) = pair[0];
        let end = pair[1].1;
        if end <= start {
            continue;
        }
        let byte = buffer.text.char_to_byte(at);
        let token = spans
            .iter()
            .find(|s| s.range.contains(&byte))
            .map(|s| s.token);
        let color = token.map_or_else(
            || {
                if source.text[start..end]
                    .chars()
                    .all(|ch| ch.is_ascii_punctuation() && ch != '_')
                {
                    theme.punctuation
                } else {
                    theme.text
                }
            },
            |token| theme.token(token),
        );
        let mut run_font = font(FONT_FAMILY);
        if theme.italic_comments && token == Some(crate::syntax::Token::Comment) {
            run_font.style = FontStyle::Italic;
        }
        if let Some(last) = runs.last_mut()
            && last.color == color
            && last.font.style == run_font.style
        {
            last.len += end - start;
        } else {
            runs.push(TextRun {
                len: end - start,
                font: run_font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    }
    let covered: usize = runs.iter().map(|r| r.len).sum();
    if covered < source.text.len() {
        runs.push(TextRun {
            len: source.text.len() - covered,
            font: font(FONT_FAMILY),
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    runs
}
fn shape(window: &Window, text: &str, font_size: f32, color: Hsla) -> ShapedLine {
    shape_weighted(window, text, font_size, color, FontWeight::NORMAL)
}
fn shape_weighted(
    window: &Window,
    text: &str,
    font_size: f32,
    color: Hsla,
    weight: FontWeight,
) -> ShapedLine {
    window.text_system().shape_line(
        text.to_owned().into(),
        px(font_size),
        &[TextRun {
            len: text.len(),
            font: Font {
                weight,
                ..font(FONT_FAMILY)
            },
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    )
}
/// Shell composition. Every region is built from [`design`] tokens and components;
/// see DESIGN.md for the layout grid and layer order.
impl EditorView {
    fn scale(&self) -> design::Scale {
        design::Scale::new(self.model.editor.config.font_size)
    }
    /// Indentation levels for each visible row. Blank lines continue the shallower
    /// of their neighbours so guides never break across empty lines. A row's
    /// `active` level marks the innermost guide of the cursor's block.
    fn indent_guides(&self, cache: &Cache) -> Guides {
        let e = &self.model.editor;
        let unit = e.indentation().width.max(1);
        let tab = e.config.tab_width.max(1);
        let measured: Vec<Option<usize>> = cache
            .lines
            .iter()
            .map(|row| {
                let mut column = 0;
                for c in e.buffer.line_slice(row.source.row).chars() {
                    match c {
                        ' ' => column += 1,
                        '\t' => column += tab - column % tab,
                        '\n' | '\r' => return None,
                        _ => return Some(column / unit),
                    }
                }
                None
            })
            .collect();
        let count = measured.len();
        let mut levels = vec![0usize; count];
        let mut before = 0;
        for i in 0..count {
            levels[i] = match measured[i] {
                Some(level) => {
                    before = level;
                    level
                }
                None => {
                    let after = measured[i + 1..]
                        .iter()
                        .flatten()
                        .next()
                        .copied()
                        .unwrap_or(0);
                    before.min(after)
                }
            };
        }
        let cursor = cache
            .lines
            .iter()
            .position(|r| r.source.row == e.buffer.row());
        let mut rows: Vec<(usize, Option<usize>)> = levels.iter().map(|l| (*l, None)).collect();
        if let Some(i) = cursor
            && let Some(level) = levels[i].checked_sub(1)
        {
            // Light the guide over the contiguous block around the cursor.
            for j in (0..=i).rev() {
                if rows[j].0 <= level {
                    break;
                }
                rows[j].1 = Some(level);
            }
            for row in rows.iter_mut().skip(i + 1) {
                if row.0 <= level {
                    break;
                }
                row.1 = Some(level);
            }
        }
        Guides { unit, rows }
    }
    /// The overview rail along the editor's right edge: a lens marking the visible
    /// span plus ticks for diagnostics (by severity) and search matches. Bounded:
    /// inputs are capped and ticks are deduplicated per pixel row.
    fn paint_rail(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let e = &self.model.editor;
        let Some(cache) = &self.cache else {
            return;
        };
        let lines = e.buffer.text.len_lines().max(1) as f32;
        let top = bounds.top() + px(space::XS);
        let height = (bounds.size.height - px(space::MD)).max(px(1.));
        let width = px(5.);
        let x = bounds.right() - width - px(space::SM);
        if lines > cache.rows as f32 {
            window.paint_quad(
                fill(
                    Bounds::new(point(x, top), size(width, height)),
                    self.theme.hover,
                )
                .corner_radii(Corners::all(width / 2.)),
            );
            let span = (cache.rows as f32 / lines).min(1.);
            let thumb = (height * span).max(px(22.));
            let progress = (e.top as f32 / (lines - cache.rows as f32).max(1.)).min(1.);
            let y = top + (height - thumb) * progress;
            window.paint_quad(
                fill(
                    Bounds::new(point(x, y), size(width, thumb)),
                    linear_gradient(
                        180.,
                        linear_color_stop(self.theme.lens_top, 0.),
                        linear_color_stop(self.theme.lens_bottom, 1.),
                    ),
                )
                .corner_radii(Corners::all(width / 2.))
                .border_widths(px(1.))
                .border_color(self.theme.lens_edge.opacity(0.5)),
            );
        }
        let rows = (f32::from(height) as usize).max(1);
        let mut seen = vec![0u8; rows];
        let mut tick = |line: usize, rank: u8, color: Hsla, window: &mut Window| {
            let at = (((line as f32 / lines) * rows as f32) as usize).min(rows - 1);
            if seen[at] >= rank {
                return;
            }
            seen[at] = rank;
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(x - px(2.), top + px(at as f32)),
                        size(width + px(4.), px(2.)),
                    ),
                    color,
                )
                .corner_radii(Corners::all(px(1.))),
            );
        };
        if e.search_revision == e.buffer.revision {
            let end = e.buffer.text.len_chars();
            for range in e.search_ranges.iter().take(2048) {
                let line = e.buffer.text.char_to_line(range.start.min(end));
                tick(line, 1, self.theme.search.opacity(0.8), window);
            }
        }
        for (_, (_, _, items)) in
            self.services
                .diagnostics
                .iter()
                .filter(|((_, id), (revision, _, _))| {
                    *id == e.document_id && *revision == e.buffer.revision
                })
        {
            for d in items.iter().take(1024) {
                let rank = match d.severity {
                    1 => 4,
                    2 => 3,
                    _ => 2,
                };
                tick(
                    d.range.start.line as usize,
                    rank,
                    self.theme.diagnostic(d.severity),
                    window,
                );
            }
        }
    }
    fn toggle_explorer(&mut self) {
        self.explorer.visible = !self.explorer.visible;
        if !self.explorer.visible {
            self.explorer.focused = false;
        }
        if self.explorer.visible && self.explorer.index.is_none() {
            self.explorer.open(
                self.explorer.root.clone(),
                self.model.editor.config.navigation.clone(),
            );
        }
    }
}
impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if !self.input_enabled() {
            return None;
        }
        let (r, text) = self.model.text_utf16(r);
        *adjusted = Some(r);
        Some(text)
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.input_enabled().then(|| UTF16Selection {
            range: self.model.selected_utf16(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.model
            .marked
            .as_ref()
            .map(|r| self.model.to_utf16(r.start)..self.model.to_utf16(r.end))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.model.unmark();
        self.changed(cx);
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_enabled() {
            let typed = self.typed_character.take();
            if r.as_ref().is_none_or(|r| {
                r.start == r.end
                    && self.model.from_utf16(r.start) == self.model.editor.buffer.cursor
            }) && self.model.marked.is_none()
                && self.model.editor.mode == Mode::Insert
                && text.chars().count() == 1
                && typed == text.chars().next()
            {
                let c = typed.expect("single typed character");
                #[cfg(feature = "perf")]
                self.services.note_completion_input(
                    self.input_started
                        .take()
                        .unwrap_or_else(std::time::Instant::now),
                );
                if !self.services.commit_character(&mut self.model, c) {
                    let token = self.services.pairing_token(&self.model);
                    self.model.editor.type_character_with_context(c, token);
                }
                self.services.update(&self.model);
                self.services.typed_trigger(&self.model, c);
            } else {
                self.model.replace_input(r, text, false, None);
            }

            self.changed(cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_enabled() {
            self.typed_character = None;
            self.dismiss_popup();
            self.model.replace_input(r, text, true, selected);
            self.changed(cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.model.editor.mode == Mode::Command {
            let line = self.command_line.as_ref()?;
            // The shaped command text has no prefix: the lens chip shows `:`/`/`.
            let byte = self
                .model
                .editor
                .command
                .chars()
                .take(self.model.from_utf16(r.start))
                .map(char::len_utf8)
                .sum::<usize>();
            let x = (line.origin.x + line.line.x_for_index(byte))
                .clamp(line.bounds.left(), line.bounds.right());
            return Some(Bounds::new(
                point(x, line.origin.y),
                size(px(2.), px(self.line_height())),
            ));
        }
        let at = self.model.from_utf16(r.start);
        let row = self
            .cache
            .as_ref()?
            .lines
            .iter()
            .find(|row| at >= row.source.start && at <= row.source.end)?;
        let x = row.shape.x_for_index(row.source.byte_for_char(at));
        Some(Bounds::new(
            point(row.origin.x + x, row.origin.y),
            size(self.cell, px(self.line_height())),
        ))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.char_at(p).map(|at| self.model.to_utf16(at))
    }
}
