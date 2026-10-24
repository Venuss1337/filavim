use super::{FONT_FAMILY, input, io, model::Model, theme::Theme, viewport::VisibleLine};
use crate::{
    editor::{Mode, Request},
    input::Action,
    key::KeyCode,
};
use gpui::{prelude::*, *};
use std::{ops::Range, path::PathBuf};

const GUTTER: f32 = 68.0;
const PADDING: f32 = 12.0;
#[derive(Clone)]
struct Row {
    source: VisibleLine,
    shape: ShapedLine,
    number: ShapedLine,
    origin: Point<Pixels>,
}
struct Cache {
    document: u64,
    revision: u64,
    top: usize,
    left: usize,
    columns: usize,
    rows: usize,
    lines: Vec<Row>,
}
pub struct EditorView {
    pub model: Model,
    focus: FocusHandle,
    theme: Theme,
    bounds: Option<Bounds<Pixels>>,
    cell: Pixels,
    gutter: f32,
    cache: Option<Cache>,
    follow_cursor: bool,
    busy: bool,
    prompt: bool,
    dragging: bool,
    scroll_y: f32,
    scroll_x: f32,
    pub painted_frames: u64,
}
impl EditorView {
    pub fn new(model: Model, cx: &mut Context<Self>) -> Self {
        let theme = Theme::new(&model.editor.config);
        Self {
            model,
            theme,
            focus: cx.focus_handle(),
            bounds: None,
            cell: px(10.),
            gutter: GUTTER,
            cache: None,
            follow_cursor: true,
            busy: false,
            prompt: false,
            dragging: false,
            scroll_y: 0.,
            scroll_x: 0.,
            painted_frames: 0,
        }
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
    pub fn smoke_summary(&self) -> String {
        format!(
            "painted_frames={} document_id={} chars={} lines={} cached_rows={} busy={} error={} message={}\n",
            self.painted_frames,
            self.model.editor.document_id,
            self.model.editor.buffer.text.len_chars(),
            self.model.editor.buffer.text.len_lines(),
            self.cache.as_ref().map_or(0, |c| c.lines.len()),
            self.busy,
            self.model.editor.error,
            self.model.editor.message
        )
    }
    pub fn open_initial(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.request(
            Request::Open {
                path,
                revision: self.model.editor.buffer.revision,
            },
            cx,
        );
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.follow_cursor = true;
        cx.notify();
    }
    fn request(&mut self, request: Request, cx: &mut Context<Self>) {
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
                    cx.quit();
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
                let Some(job) = io::Job::from_request(r, self.model.editor.document_id) else {
                    return;
                };
                self.busy = true;
                let task = cx.background_executor().spawn(async move { job.run() });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    let _ = view.update(cx, |view, cx| {
                        view.busy = false;
                        match result {
                            Ok(done) => {
                                let before = view.model.editor.document_id;
                                let quit = io::apply(&mut view.model.editor, done);
                                if before != view.model.editor.document_id {
                                    view.model.unmark();
                                    view.model.keymap.clear();
                                    view.cache = None;
                                }
                                if quit {
                                    cx.quit();
                                }
                            }
                            Err(error) => view.model.editor.notify(format!("{error:#}"), true),
                        }
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
                    _ => {}
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
        if self.model.editor.buffer.dirty() {
            self.model
                .editor
                .notify("Save changes first, or use :e! <path> to discard", true);
            cx.notify();
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
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.busy {
            self.model
                .editor
                .notify("Wait for the file operation before closing", true);
            cx.notify();
            return false;
        }
        if !self.model.editor.buffer.dirty() {
            return true;
        }
        if self.prompt {
            return false;
        }
        self.prompt = true;
        let document = self.model.editor.document_id;
        let revision = self.model.editor.buffer.revision;
        let receiver = window.prompt(
            PromptLevel::Warning,
            "Save changes before closing?",
            Some("Unsaved changes will be lost if you discard them."),
            &["Save", "Discard", "Cancel"],
            cx,
        );
        cx.spawn(async move |view, cx| {
            let answer = receiver.await.ok();
            let _ = view.update(cx, |view, cx| {
                view.prompt = false;
                if view.model.editor.document_id != document
                    || view.model.editor.buffer.revision != revision
                {
                    return;
                }
                match answer {
                    Some(0) => view.save(true, false, cx),
                    Some(1) => cx.quit(),
                    _ => {}
                }
            });
        })
        .detach();
        false
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if self.prompt {
            return;
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
                "v" if self.model.input_enabled() || self.model.editor.mode == Mode::Visual => {
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
        let (handled, request) = self.model.key(key);
        if let Some(request) = request {
            self.request(request, cx);
        }
        if handled || !self.model.input_enabled() {
            cx.stop_propagation();
            window.prevent_default();
            self.changed(cx);
        }
    }
    fn mouse(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        if self.prompt || self.model.editor.mode == Mode::Command {
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
                && c.revision == e.buffer.revision
                && c.top == e.top
                && c.left == e.left
                && c.columns == columns
                && c.rows == rows
        });
        if !valid {
            let mut lines = Vec::with_capacity(rows);
            for row in e.top..(e.top + rows).min(e.buffer.text.len_lines()) {
                let source = VisibleLine::new(&e.buffer, row, e.left, columns, e.config.tab_width);
                let shaped = shape(window, &source.text, e.config.font_size, self.theme.text);
                let number = shape(
                    window,
                    &(row + 1).to_string(),
                    e.config.font_size - 2.,
                    self.theme.muted,
                );
                lines.push(Row {
                    source,
                    shape: shaped,
                    number,
                    origin: point(px(0.), px(0.)),
                });
            }
            self.cache = Some(Cache {
                document: e.document_id,
                revision: e.buffer.revision,
                top: e.top,
                left: e.left,
                columns,
                rows,
                lines,
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
        let _ = cx;
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
        let cursor = e.buffer.cursor;
        window.with_content_mask(Some(mask), |window| {
            for row in &cache.lines {
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
                if self.focus.is_focused(window)
                    && e.mode != Mode::Command
                    && e.buffer.row() == row.source.row
                    && cursor >= row.source.start
                    && cursor <= row.source.end
                {
                    let x = row.shape.x_for_index(row.source.byte_for_char(cursor));
                    let width = if e.mode == Mode::Insert {
                        px(2.)
                    } else {
                        self.cell
                    };
                    let y = if e.mode == Mode::Insert {
                        row.origin.y
                    } else {
                        row.origin.y + px(self.line_height() - 3.)
                    };
                    let h = if e.mode == Mode::Insert {
                        px(self.line_height())
                    } else {
                        px(3.)
                    };
                    window.paint_quad(fill(
                        Bounds::new(point(row.origin.x + x, y), size(width, h)),
                        self.theme.accent,
                    ));
                }
            }
        });
        if e.config.line_numbers {
            for row in &cache.lines {
                let origin = point(
                    bounds.left() + px(gutter - 14.) - row.number.width,
                    row.origin.y,
                );
                if let Err(error) = row.number.paint(origin, px(self.line_height()), window, cx) {
                    eprintln!("Gutter paint: {error}");
                }
            }
        }
    }
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pre = cx.entity();
        let paint = cx.entity();
        let e = &self.model.editor;
        let path = e
            .buffer
            .path
            .as_ref()
            .map_or_else(|| "[No Name]".into(), |p| p.display().to_string());
        let status = format!(
            "{}{}   {}   {}:{}{}",
            e.mode.label(),
            if e.buffer.dirty() { " [+]" } else { "" },
            path,
            e.buffer.row() + 1,
            e.buffer.column(e.config.tab_width) + 1,
            if self.busy { "   Working…" } else { "" }
        );
        let command = if e.mode == Mode::Command {
            format!("{}{}▏", if e.searching { "/" } else { ":" }, e.command)
        } else if e.help {
            "W/A/S/D move · I insert · V visual · U undo · Ctrl-R redo · :e path · :w · :q · Esc close help".into()
        } else {
            e.message.clone()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.base)
            .text_color(self.theme.text)
            .font_family(FONT_FAMILY)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(
                div()
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
            )
            .child(
                div()
                    .h(px(self.line_height()))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(self.theme.surface)
                    .text_size(px(e.config.font_size))
                    .line_height(px(self.line_height()))
                    .child(status),
            )
            .child(
                div()
                    .h(px(self.line_height()))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .px(px(12.))
                    .text_size(px(e.config.font_size))
                    .line_height(px(self.line_height()))
                    .text_color(if e.error {
                        self.theme.error
                    } else {
                        self.theme.muted
                    })
                    .child(command),
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
        if !self.model.input_enabled() {
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
        self.model.input_enabled().then(|| UTF16Selection {
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
        if self.model.input_enabled() {
            self.model.replace_input(r, text, false, None);
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
        if self.model.input_enabled() {
            self.model.replace_input(r, text, true, selected);
            self.changed(cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        if self.model.editor.mode == Mode::Command {
            return Some(Bounds::new(
                point(
                    bounds.left()
                        + px(PADDING)
                        + shape(
                            window,
                            &format!(
                                "{}{}",
                                if self.model.editor.searching {
                                    "/"
                                } else {
                                    ":"
                                },
                                self.model
                                    .editor
                                    .command
                                    .chars()
                                    .take(self.model.from_utf16(r.start))
                                    .collect::<String>()
                            ),
                            self.model.editor.config.font_size,
                            self.theme.text,
                        )
                        .width,
                    bounds.bottom() + px(self.line_height()),
                ),
                size(self.cell, px(self.line_height())),
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
