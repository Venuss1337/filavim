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
use super::{FONT_FAMILY, input, io, model::Model, theme::Theme, viewport::VisibleLine};
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
const GUTTER: f32 = 68.0;
const PADDING: f32 = 12.0;
#[derive(Clone)]
struct Row {
    source: VisibleLine,
    shape: ShapedLine,
    number: ShapedLine,
    active_number: ShapedLine,
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
    end_marker: ShapedLine,
    search: String,
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
    wallpaper: Entity<super::wallpaper::Wallpaper>,
    bounds: Option<Bounds<Pixels>>,
    cell: Pixels,
    gutter: f32,
    cache: Option<Cache>,
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
        let wallpaper =
            cx.new(|cx| super::wallpaper::Wallpaper::new(&model.editor.config, theme, cx));
        cx.subscribe(
            &wallpaper,
            |view, _, error: &super::wallpaper::WallpaperError, cx| {
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
            wallpaper,
            focus,
            bounds: None,
            cell: px(10.),
            gutter: GUTTER,
            cache: None,
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
    /// Explicit bounded real-server smoke mode; mutates only the in-memory fixture.
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
                            v.wallpaper.update(cx, |wallpaper, cx| {
                                wallpaper.configure(&config, v.theme, false, cx)
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
                self.wallpaper.update(cx, |wallpaper, cx| {
                    wallpaper.configure(&config, self.theme, true, cx)
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
            "Explorer" => {
                self.explorer.visible = !self.explorer.visible;
                if self.explorer.visible && self.explorer.index.is_none() {
                    self.explorer.open(
                        self.explorer.root.clone(),
                        self.model.editor.config.navigation.clone(),
                    );
                }
            }
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
                let runs = syntax_runs(&source, &e.buffer, &spans, &search, &self.theme);
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
                let active_number = shape(
                    window,
                    &(row + 1).to_string(),
                    e.config.font_size - 2.,
                    self.theme.accent,
                );
                lines.push(Row {
                    search,
                    source,
                    shape: shaped,
                    number,
                    active_number,
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
                end_marker: shape(window, "~", e.config.font_size - 2., self.theme.muted),
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
        self.cursor.update(cx, |cursor, cx| {
            cursor.update_target(target, scene, &e.config.cursor, self.theme.cursor, clip, cx)
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
        let color = if e.error {
            self.theme.error
        } else {
            self.theme.secondary
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
        self.cursor.update(cx, |cursor, cx| {
            cursor.update_target(
                target,
                scene,
                &e.config.cursor,
                self.theme.cursor,
                bounds,
                cx,
            )
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
        let panel = self.services.panel.as_ref()?;
        if panel.request != request {
            return None;
        }
        let row = panel.rows.get(i)?;
        let mut highlights = Vec::new();
        if let super::services::Choice::Completion(value) = &row.choice {
            let end = row.label.rfind("  · ").unwrap_or(row.label.len());
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
                            color: Some(self.theme.matched),
                            ..Default::default()
                        },
                    )
                }),
            );
            if end < row.label.len() {
                highlights.push((
                    end..row.label.len(),
                    HighlightStyle {
                        color: Some(
                            self.theme
                                .completion_kind(value["kind"].as_u64().unwrap_or(0) as usize),
                        ),
                        ..Default::default()
                    },
                ));
            }
        }
        let label = StyledText::new(row.label.clone()).with_highlights(highlights);
        Some(
            div()
                .id(("popup-row", i))
                .h(px(row_height))
                .px(px(10.))
                .py(px(3.))
                .whitespace_nowrap()
                .text_ellipsis()
                .overflow_hidden()
                .bg(if i == panel.selected {
                    self.theme.popup_selection
                } else {
                    self.theme.overlay.opacity(0.)
                })
                .cursor_pointer()
                .child(label)
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
        if let Some(confirm) = &self.confirmation {
            let selected = confirm.selected;
            let name = self
                .model
                .get(confirm.document)
                .and_then(|e| e.buffer.path.as_ref())
                .map_or_else(|| "Untitled".into(), |p| p.display().to_string());
            let (title, detail, labels): (&str, &str, &[&str]) = match &confirm.decision {
                Decision::Workspace { .. } => (
                    "Apply workspace edit?",
                    "Approval can change files on disk. Text edits stay unsaved and undoable. Reject or Escape leaves both unchanged. D shows the ordered changes.",
                    &["Approve", "Reject", "Cancel"],
                ),
                Decision::Recovery(_) => (
                    "Recover unsaved documents?",
                    "Recovery opens local snapshots without overwriting disk. Later keeps the stored copies.",
                    &["Recover", "Discard", "Later"],
                ),
                Decision::Close => (
                    "Save changes before closing?",
                    "Discard loses unsaved changes. Escape cancels.",
                    &["Save", "Discard", "Cancel"],
                ),
                Decision::Conflict {
                    overwrite: false, ..
                } => (
                    "File changed outside FilaVim",
                    "Reload discards editor changes. Keep preserves both versions. Saving a copy leaves the original intact.",
                    &["Reload", "Keep", "Save copy", "Overwrite…"],
                ),
                Decision::Conflict {
                    overwrite: true, ..
                } => (
                    "Overwrite external changes?",
                    "This replaces the disk version with your editor contents. Further disk changes will stop the save.",
                    &["Overwrite", "Cancel"],
                ),
            };
            let name = match &confirm.decision {
                Decision::Conflict { path, actual, .. } => {
                    let disk = match actual {
                        crate::files::DiskState::Missing => "disk file is missing".into(),
                        crate::files::DiskState::Present { bytes, .. } => {
                            format!("disk: {bytes} bytes")
                        }
                    };
                    let editor = self
                        .model
                        .get(confirm.document)
                        .map_or(0, |e| e.buffer.text.len_bytes());
                    format!("{} · {disk} · editor: {editor} bytes", path.display())
                }
                Decision::Recovery(entries) => format!(
                    "{} documents: {}",
                    entries.len(),
                    entries
                        .iter()
                        .take(4)
                        .map(|e| e
                            .path
                            .as_ref()
                            .map_or_else(|| "Untitled".into(), |p| p.display().to_string()))
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
            let expanded = matches!(confirm.decision, Decision::Workspace { expanded: true, .. });
            let area = place(
                full,
                None,
                (720., if expanded { 460. } else { 280. }),
                false,
            )
            .menu;
            let workspace_details: Option<AnyElement> =
                if let Decision::Workspace {
                    token, position, ..
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
                            if expanded {
                                let selected = entries.get(position).cloned().unwrap_or_default();
                                let background = self.theme.selection;
                                let list = uniform_list(
                                    "workspace-operations",
                                    entries.len(),
                                    move |range, _, _| {
                                        range
                                            .map(|i| {
                                                div()
                                                    .h(px(28.))
                                                    .text_size(px(12.))
                                                    .when(i == position, |d| d.bg(background))
                                                    .overflow_hidden()
                                                    .child(entries[i].clone())
                                                    .into_any_element()
                                            })
                                            .collect()
                                    },
                                )
                                .track_scroll(self.workspace_scroll.clone())
                                .flex_1()
                                .min_h(px(0.));
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .gap(px(8.))
                                    .child(list)
                                    .child(
                                        div()
                                            .id("workspace-operation-details")
                                            .max_h(px(88.))
                                            .overflow_y_scroll()
                                            .text_size(px(12.))
                                            .child(selected),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(px(12.))
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
            let buttons = labels
                .iter()
                .copied()
                .enumerate()
                .map(|(i, label)| {
                    div()
                        .id(("confirm-choice", i))
                        .px(px(16.))
                        .py(px(8.))
                        .border_1()
                        .border_color(self.theme.border)
                        .bg(if i == selected {
                            self.theme.selection
                        } else {
                            self.theme.overlay
                        })
                        .cursor_pointer()
                        .child(label)
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
            let panel = div()
                .absolute()
                .left(px(area.x))
                .top(px(area.y))
                .w(px(area.width))
                .h(px(area.height))
                .bg(self.theme.overlay)
                .border_1()
                .border_color(self.theme.border)
                .p(px(18.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(title)
                .child(
                    div()
                        .text_color(self.theme.secondary)
                        .overflow_hidden()
                        .child(name),
                )
                .child(div().child(detail))
                .children(workspace_details)
                .child(div().flex().gap(px(10.)).children(buttons));
            let backdrop = div()
                .w(viewport.width)
                .h(viewport.height)
                .font_family(FONT_FAMILY)
                .text_size(px(self.model.editor.config.font_size))
                .text_color(self.theme.text)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(panel)
                .into_any_element();
            self.popup_frames += 1;
            return vec![(backdrop, point(px(0.), px(0.)))];
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
        let font_size = self.model.editor.config.font_size.min(20.);
        let row_height = (font_size * 1.5).ceil() + 6.;
        let detail = panel
            .rows
            .get(selected)
            .map(|r| r.detail.clone())
            .filter(|s| !s.is_empty());
        let placement = place(
            bounds,
            caret,
            (
                if contextual { 450. } else { 720. },
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
                    } else {
                        260.
                    }
                } else if presentation == Presentation::Completion {
                    8. * row_height + 14.
                } else {
                    panel.rows.len().clamp(1, 8) as f32 * row_height + 64.
                },
            ),
            detail.is_some() && !self.documentation_only,
        );
        let title = if self.documentation_only {
            "Documentation · F1/Esc returns to suggestions".into()
        } else {
            panel.title.clone()
        };
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
                        self.theme,
                    ))
                    .into_any_element(),
            ]
        } else if information {
            panel
                .rows
                .iter()
                .take(100)
                .map(|r| {
                    let color = if panel.title == "Diagnostics" {
                        if r.label.starts_with("[Error]") {
                            self.theme.error
                        } else if r.label.starts_with("[Warning]") {
                            self.theme.warning
                        } else if r.label.starts_with("[Information]") {
                            self.theme.info
                        } else if r.label.starts_with("[Hint]") {
                            self.theme.hint
                        } else {
                            self.theme.text
                        }
                    } else {
                        self.theme.text
                    };
                    div()
                        .mb(px(6.))
                        .text_color(color)
                        .child(match &r.choice {
                            super::services::Choice::Signature(ranges) => {
                                super::floating::signature(&r.label, ranges, self.theme)
                            }
                            _ => super::floating::documentation(r.label.clone(), self.theme),
                        })
                        .into_any_element()
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
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .track_scroll(&self.popup_scroll)
                .p(px(8.))
                .children(rows)
                .into_any_element()
        } else {
            let entity = cx.entity();
            let request = panel.request;
            let count = panel.rows.len();
            uniform_list("popup-list", count, move |range, _, cx| {
                entity.update(cx, |v, cx| {
                    range
                        .filter_map(|i| v.popup_row(i, row_height, request, cx))
                        .collect()
                })
            })
            .track_scroll(self.popup_list_scroll.clone())
            .flex_1()
            .min_h(px(0.))
            .into_any_element()
        };
        let icon = if presentation == Presentation::Completion && !self.documentation_only {
            "⚡"
        } else if panel.title == "Diagnostics" {
            "!"
        } else {
            "i"
        };
        let surface = super::floating::frame(self.theme, "floating-popup", font_size)
            .w(dimensions.width)
            .h(dimensions.height)
            .flex()
            .flex_col()
            .when(minimal, |d| {
                d.py(px(6.))
                    .child(super::floating::indicator(self.theme, icon))
            })
            .when(!minimal, |d| {
                d.child(
                    div()
                        .h(px(row_height))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .flex_shrink_0()
                        .px(px(10.))
                        .py(px(4.))
                        .text_color(self.theme.accent)
                        .child(title),
                )
            })
            .child(body)
            .when(
                !information && presentation != Presentation::Completion,
                |d| {
                    d.child(
                        div()
                            .h(px(24.))
                            .flex_shrink_0()
                            .px(px(10.))
                            .text_size(px((font_size - 2.).max(12.)))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(self.theme.secondary)
                            .child(format!(
                                "{} / {} · ↑↓ select · {} · Esc close{}{}",
                                selected + 1,
                                panel.rows.len(),
                                if matches!(
                                    presentation,
                                    Presentation::Picker | Presentation::ContextMenu
                                ) || self.model.editor.config.completion.accept_enter
                                    && self.model.editor.config.completion.accept_tab
                                {
                                    "Enter/Tab accept"
                                } else if self.model.editor.config.completion.accept_enter {
                                    "Enter accepts"
                                } else if self.model.editor.config.completion.accept_tab {
                                    "Tab accepts"
                                } else {
                                    "click accepts"
                                },
                                if presentation == Presentation::Completion {
                                    " · F1 docs"
                                } else {
                                    ""
                                },
                                if self.services.resolving_selected() {
                                    " · resolving…"
                                } else {
                                    ""
                                }
                            )),
                    )
                },
            )
            .into_any_element();
        let mut draws = vec![(surface, origin)];
        if let (Some(detail), Some(area)) = (detail, placement.documentation) {
            let origin = point(px(area.x), px(area.y));
            let dimensions = size(px(area.width), px(area.height));
            self.popup_bounds.push(Bounds::new(origin, dimensions));
            draws.push((
                super::floating::frame(self.theme, "completion-documentation", font_size)
                    .w(dimensions.width)
                    .h(dimensions.height)
                    .child(super::floating::indicator(self.theme, "i"))
                    .child(
                        div()
                            .id("completion-documentation-content")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.documentation_scroll)
                            .p(px(10.))
                            .child(super::floating::documentation(detail, self.theme)),
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
                lines as f32 * font_size * 1.5 + 20.,
            ) {
                let origin = point(px(area.x), px(area.y));
                let dimensions = size(px(area.width), px(area.height));
                self.popup_bounds.push(Bounds::new(origin, dimensions));
                let rows = signature.rows.iter().take(2).map(|row| {
                    let text = match &row.choice {
                        super::services::Choice::Signature(active) => {
                            super::floating::signature(&row.label, active, self.theme)
                        }
                        _ => StyledText::new(row.label.clone()),
                    };
                    div().child(text)
                });
                draws.push((
                    super::floating::frame(self.theme, "signature-assistance", font_size)
                        .w(dimensions.width)
                        .h(dimensions.height)
                        .child(super::floating::indicator(self.theme, "i"))
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.))
                                .overflow_hidden()
                                .p(px(10.))
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
        window.with_content_mask(Some(mask), |window| {
            for row in &cache.lines {
                if e.buffer.row() == row.source.row {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.left(), row.origin.y),
                            size(bounds.size.width, px(self.line_height())),
                        ),
                        self.theme.active_line,
                    ));
                }
                for &(start, end) in &row.search {
                    let left = row.shape.x_for_index(start);
                    let right = row.shape.x_for_index(end);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(row.origin.x + left, row.origin.y),
                            size((right - left).max(px(1.)), px(self.line_height())),
                        ),
                        if e.search_revision == e.buffer.revision
                            && e.search_ranges
                                .binary_search_by_key(&e.buffer.cursor, |r| r.start)
                                .ok()
                                .is_some_and(|i| {
                                    let r = &e.search_ranges[i];
                                    r.start == e.buffer.cursor
                                        && r.start >= row.source.start
                                        && r.start <= row.source.end
                                        && row.source.byte_for_char(r.start) == start
                                })
                        {
                            self.theme.selection
                        } else {
                            self.theme.search
                        },
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
                    window.paint_quad(fill(
                        Bounds::new(
                            point(row.origin.x + start, row.origin.y),
                            size((end - start).max(px(1.)), px(self.line_height())),
                        ),
                        self.theme.selection,
                    ));
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
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.left() + px(4.), row.origin.y + px(6.)),
                            size(px(4.), px(10.)),
                        ),
                        self.theme.diagnostic(diagnostic.severity),
                    ));
                }
                let active = e.buffer.row() == row.source.row;
                let number = if active {
                    &row.active_number
                } else {
                    &row.number
                };
                let origin = point(
                    if active {
                        bounds.left() + px(14.)
                    } else {
                        bounds.left() + px(gutter - 14.) - number.width
                    },
                    row.origin.y,
                );
                if let Err(error) = number.paint(origin, px(self.line_height()), window, cx) {
                    eprintln!("Gutter paint: {error}");
                }
            }
            for index in cache.lines.len()..cache.rows {
                let origin = point(
                    bounds.left() + px(14.),
                    bounds.top() + index as f32 * px(self.line_height()),
                );
                if let Err(error) =
                    cache
                        .end_marker
                        .paint(origin, px(self.line_height()), window, cx)
                {
                    eprintln!("End-of-file gutter paint: {error}");
                }
            }
        }
    }
}
fn syntax_runs(
    source: &VisibleLine,
    buffer: &crate::buffer::Buffer,
    spans: &[crate::syntax::Highlight],
    search: &[(usize, usize)],
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
        let mut color = token.map_or_else(
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
        if search.iter().any(|&(a, b)| start >= a && start < b) {
            color = theme.search_text;
        }
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
    window.text_system().shape_line(
        text.to_owned().into(),
        px(font_size),
        &[TextRun {
            len: text.len(),
            font: font(FONT_FAMILY),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    )
}
impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar_width = self
            .explorer
            .width
            .min((f32::from(window.viewport_size().width) - 240.).max(100.));
        let pre = cx.entity();
        let paint = cx.entity();
        let overlay = cx.entity();
        let command_pre = cx.entity();
        let command_paint = cx.entity();
        let title_height = self.line_height();
        let tabs = self
            .model
            .order
            .iter()
            .map(|id| {
                let editor = self.model.get(*id).expect("live document");
                let dirty = editor.buffer.dirty();
                let label = editor
                    .buffer
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or_else(|| "Untitled".into(), |n| n.to_string_lossy().into_owned());
                let id = *id;
                let active = id == self.model.id();
                div()
                    .id(("tab", id))
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .h(px(title_height))
                    .min_w(px(80.))
                    .max_w(px(240.))
                    .flex_shrink_0()
                    .px(px(8.))
                    .border_b_1()
                    .border_color(if active {
                        self.theme.accent
                    } else {
                        self.theme.border
                    })
                    .bg(if active {
                        self.theme.active_tab
                    } else {
                        self.theme.chrome
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, _, w, cx| {
                            v.model.switch(id);
                            v.explorer.focused = false;
                            v.focus(w, cx);
                            v.changed(cx);
                        }),
                    )
                    .text_color(if active {
                        self.theme.text
                    } else {
                        self.theme.secondary
                    })
                    .hover(|d| d.bg(self.theme.selection))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(label),
                    )
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_color(self.theme.accent)
                                .child("●"),
                        )
                    })
                    .child(
                        div()
                            .id(("close", id))
                            .w(px(18.))
                            .flex_shrink_0()
                            .text_center()
                            .hover(|d| d.bg(self.theme.border))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, _, w, cx| {
                                    cx.stop_propagation();
                                    v.close_document(id, w, cx);
                                }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let explorer = if self.explorer.visible {
            let count = self.explorer.rows.len();
            let entity = cx.entity();
            Some(
                div()
                    .w(px(sidebar_width))
                    .h_full()
                    .flex_shrink_0()
                    .bg(self.theme.sidebar)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(8.))
                            .text_color(self.theme.secondary)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.explorer.root.file_name().map_or_else(
                                || "Project".into(),
                                |n| n.to_string_lossy().into_owned(),
                            )),
                    )
                    .child(
                        uniform_list("explorer", count, move |range, _, cx| {
                            entity.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let row = &v.explorer.rows[i];
                                        let label = format!(
                                            "{}{}",
                                            if row.entry.directory {
                                                if row.expanded { "▾ " } else { "▸ " }
                                            } else {
                                                "  "
                                            },
                                            row.entry.path.file_name().map_or_else(
                                                || row.entry.label.clone(),
                                                |n| n.to_string_lossy().into_owned()
                                            )
                                        );
                                        let active = v.model.editor.buffer.path.as_ref()
                                            == Some(&row.entry.path);
                                        div()
                                            .id(("file", i))
                                            .h(px(29.))
                                            .py(px(3.))
                                            .pr(px(8.))
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(if row.entry.directory {
                                                v.theme.secondary
                                            } else {
                                                v.theme.text
                                            })
                                            .hover(|d| d.bg(v.theme.selection))
                                            .pl(px(10. + row.depth as f32 * 14.))
                                            .overflow_hidden()
                                            .bg(
                                                if active
                                                    || v.explorer.focused
                                                        && v.explorer.selected == i
                                                {
                                                    v.theme.selection
                                                } else {
                                                    v.theme.sidebar.opacity(0.)
                                                },
                                            )
                                            .cursor_pointer()
                                            .child(label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |v, _, w, cx| {
                                                    v.explorer.focused = true;
                                                    v.focus(w, cx);
                                                    if let Some(path) = v.explorer.activate(i) {
                                                        v.explorer.focused = false;
                                                        v.request(
                                                            Request::Open {
                                                                path,
                                                                revision: v
                                                                    .model
                                                                    .editor
                                                                    .buffer
                                                                    .revision,
                                                            },
                                                            cx,
                                                        );
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            })
                        })
                        .track_scroll(self.explorer_scroll.clone())
                        .flex_1(),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        let e = &self.model.editor;
        let path = e
            .buffer
            .path
            .as_ref()
            .map_or_else(|| "[No Name]".into(), |p| p.display().to_string());
        let diagnostic_count: usize = self
            .services
            .diagnostics
            .iter()
            .filter(|((_, id), (revision, _, _))| {
                *id == e.document_id && *revision == e.buffer.revision
            })
            .map(|(_, (_, _, d))| d.len())
            .sum();
        let search_status = if e.last_search.is_empty() {
            String::new()
        } else if e.search_error {
            " · invalid search".into()
        } else if e.search_revision != e.buffer.revision {
            " · updating search…".into()
        } else {
            let position = e
                .search_ranges
                .binary_search_by_key(&e.buffer.cursor, |r| r.start)
                .ok()
                .map_or(0, |i| i + 1);
            format!(" · {position}/{} matches", e.search_ranges.len())
        };
        let location = format!(
            "{}:{}  ·  {} diagnostics{}",
            e.buffer.row() + 1,
            e.buffer.column(e.config.tab_width) + 1,
            diagnostic_count,
            search_status
        );
        let status = format!(
            "{}{}{}",
            path,
            if e.buffer.dirty() { "  [+]" } else { "" },
            if let Some(status) = self.services.workspace_status() {
                format!("  ·  {status}")
            } else if self.busy || self.tool_busy {
                "  ·  Working…".into()
            } else {
                String::new()
            }
        );
        let command = if e.mode == Mode::Command {
            format!("{}{}", if e.searching { "/" } else { ":" }, e.command)
        } else if e.help {
            "W/A/S/D move · I insert · V visual · U undo · Ctrl-R redo · :e path · :w · :q · Esc close help".into()
        } else {
            e.message.clone()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.base)
            .child(
                div().absolute().top(px(0.)).left(px(0.)).size_full().child(
                    AnyView::from(self.wallpaper.clone())
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .text_color(self.theme.text)
            .font_family(FONT_FAMILY)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, event: &MouseDownEvent, _, cx| {
                    if v.confirmation.is_none()
                        && v.services.panel.is_some()
                        && !v.popup_bounds.iter().any(|b| b.contains(&event.position))
                    {
                        v.dismiss_popup();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|v, event: &MouseMoveEvent, window, cx| {
                if let Some(start) = v.title_drag {
                    if event.pressed_button != Some(MouseButton::Left) {
                        v.title_drag = None;
                    } else if (event.position.x - start.x).abs() > px(4.)
                        || (event.position.y - start.y).abs() > px(4.)
                    {
                        v.title_drag = None;
                        super::application::drag_window(window);
                    }
                }
                if v.resizing {
                    v.explorer.width = f32::from(event.position.x).clamp(140., 600.);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, _| {
                    v.resizing = false;
                    v.title_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, _| {
                    v.resizing = false;
                    v.title_drag = None;
                }),
            )
            .child(
                div()
                    .id("title-bar")
                    .flex()
                    .h(px(title_height))
                    .flex_shrink_0()
                    .bg(self.theme.chrome)
                    .text_size(px(e.config.font_size))
                    .line_height(px(title_height))
                    .child(
                        div()
                            .id("document-tabs")
                            .flex()
                            .flex_shrink()
                            .min_w(px(0.))
                            .max_w_full()
                            .overflow_x_scroll()
                            .children(tabs),
                    )
                    .child(
                        div()
                            .id("title-bar-drag")
                            .flex_1()
                            .min_w(px(32.))
                            .h_full()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|v, event: &MouseDownEvent, _, cx| {
                                    v.title_drag = Some(event.position);
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("close-application")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(title_height))
                            .h_full()
                            .flex_shrink_0()
                            .cursor_pointer()
                            .text_color(self.theme.secondary)
                            .hover(|d| d.bg(self.theme.selection).text_color(self.theme.text))
                            .child("×")
                            .on_click(cx.listener(|v, _, w, cx| {
                                cx.stop_propagation();
                                v.close(w, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .children(explorer)
                    .when(self.explorer.visible, |d| {
                        d.child(div().w(px(3.)).h_full().flex_shrink_0().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|v, _, _, _| v.resizing = true),
                        ))
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .cursor(CursorStyle::IBeam)
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse))
                            .on_mouse_move(cx.listener(Self::mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|view, _, _, _| view.dragging = false),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|view, _, _, _| view.dragging = false),
                            )
                            .on_scroll_wheel(cx.listener(Self::wheel))
                            .child(
                                canvas(
                                    move |b, w, cx| {
                                        pre.update(cx, |view, cx| view.prepare(b, w, cx));
                                    },
                                    move |b, (), w, cx| {
                                        let focus = paint.read(cx).focus.clone();
                                        w.handle_input(
                                            &focus,
                                            ElementInputHandler::new(b, paint.clone()),
                                            cx,
                                        );
                                        paint.update(cx, |view, cx| view.paint(b, w, cx));
                                    },
                                )
                                .size_full(),
                            ),
                    ),
            )
            .children(self.manager_notice(cx))
            .child(
                div()
                    .h(px(self.line_height()))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(self.theme.chrome)
                    .text_size(px(e.config.font_size))
                    .line_height(px(self.line_height()))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(8.))
                            .text_color(self.theme.mode(e.mode))
                            .child(format!("-- {} --", e.mode.label())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(status),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(f32::from(window.viewport_size().width) * 0.55))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(self.theme.secondary)
                            .child(location),
                    ),
            )
            .child(
                div()
                    .h(px(self.line_height()))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .px(px(12.))
                    .text_size(px(e.config.font_size))
                    .line_height(px(self.line_height()))
                    .bg(self.theme.chrome)
                    .text_color(if e.error {
                        self.theme.error
                    } else {
                        self.theme.secondary
                    })
                    .when(e.mode == Mode::Command, |d| {
                        let text = command.clone();
                        d.child(
                            canvas(
                                move |bounds, window, cx| {
                                    command_pre.update(cx, |view, cx| {
                                        view.prepare_command(bounds, text, window, cx)
                                    })
                                },
                                move |_, (), window, cx| {
                                    command_paint
                                        .update(cx, |view, cx| view.paint_command(window, cx))
                                },
                            )
                            .size_full(),
                        )
                    })
                    .when(e.mode != Mode::Command, |d| d.child(command)),
            )
            .child(
                canvas(
                    move |_, window, cx| {
                        let draws = overlay.update(cx, |view, cx| view.popups(window, cx));
                        for (mut element, origin) in draws {
                            let viewport = window.viewport_size();
                            element.layout_as_root(
                                size(
                                    AvailableSpace::Definite(viewport.width),
                                    AvailableSpace::Definite(viewport.height),
                                ),
                                window,
                                cx,
                            );
                            window.defer_draw(element, origin, 20);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full()
                .top(px(0.))
                .left(px(0.)),
            )
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
            let byte = 1 + self
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
