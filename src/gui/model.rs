//! Per-document input/IME adapter around the existing editor.
//!
//! [`super::documents::Documents`] owns multiple models; this model wraps exactly
//! one editor. GPUI text-input offsets are UTF-16, whereas buffer positions are
//! scalar indices. Composition state belongs here, not in a second text buffer.
use crate::{
    buffer::Buffer,
    config::Config,
    editor::{Editor, Mode, Request},
    input::{Action, Keymap},
    key::KeyEvent,
};
use std::{borrow::Cow, ops::Range};

pub struct Model {
    pub editor: Editor,
    pub keymap: Keymap,
    pub marked: Option<Range<usize>>,
    pub ime_selection: Option<Range<usize>>,
}
impl Model {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            keymap: Keymap::new(&config)?,
            editor: Editor::new(config),
            marked: None,
            ime_selection: None,
        })
    }
    pub fn key(&mut self, key: KeyEvent) -> (bool, Option<Request>) {
        if self.marked.is_some() && key.code != crate::key::KeyCode::Esc {
            return (false, None);
        }
        let Some(action) = self.keymap.resolve(self.editor.mode, key) else {
            return (false, None);
        };
        self.marked = None;
        self.ime_selection = None;
        (true, self.editor.act(action))
    }
    pub fn selection(&self) -> Range<usize> {
        self.ime_selection
            .clone()
            .or_else(|| self.editor.selection())
            .unwrap_or(self.editor.buffer.cursor..self.editor.buffer.cursor)
    }
    pub fn input_enabled(&self) -> bool {
        matches!(self.editor.mode, Mode::Insert | Mode::Command) && !self.editor.help
    }
    pub fn text_len_utf16(&self) -> usize {
        if self.editor.mode == Mode::Command {
            self.editor.command.encode_utf16().count()
        } else {
            self.editor.buffer.text.len_utf16_cu()
        }
    }
    pub fn to_utf16(&self, at: usize) -> usize {
        if self.editor.mode == Mode::Command {
            self.editor
                .command
                .chars()
                .take(at)
                .map(char::len_utf16)
                .sum()
        } else {
            self.editor
                .buffer
                .text
                .char_to_utf16_cu(at.min(self.editor.buffer.text.len_chars()))
        }
    }
    pub fn from_utf16(&self, at: usize) -> usize {
        let at = at.min(self.text_len_utf16());
        if self.editor.mode == Mode::Command {
            let mut units = 0;
            self.editor
                .command
                .chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= at
                })
                .count()
        } else {
            self.editor.buffer.text.utf16_cu_to_char(at)
        }
    }
    fn range_from_utf16(&self, r: Range<usize>) -> Range<usize> {
        let start = self.from_utf16(r.start);
        let mut end = self.from_utf16(r.end.max(r.start));
        if r.end > r.start && self.to_utf16(end) < r.end.min(self.text_len_utf16()) {
            end += 1;
        }
        start..end
    }
    pub fn selected_utf16(&self) -> Range<usize> {
        if self.editor.mode == Mode::Command {
            let n = self.text_len_utf16();
            n..n
        } else {
            let r = self.selection();
            self.to_utf16(r.start)..self.to_utf16(r.end)
        }
    }
    pub fn text_utf16(&self, range: Range<usize>) -> (Range<usize>, String) {
        let r = self.range_from_utf16(range);
        let (start, end) = (r.start, r.end);
        let text = if self.editor.mode == Mode::Command {
            self.editor
                .command
                .chars()
                .skip(start)
                .take(end - start)
                .collect()
        } else {
            self.editor.buffer.text.slice(start..end).to_string()
        };
        (self.to_utf16(start)..self.to_utf16(end), text)
    }
    pub fn replace_input(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        composing: bool,
        selected: Option<Range<usize>>,
    ) {
        if !self.input_enabled() {
            return;
        }
        if self.editor.mode == Mode::Command {
            // Command input is append-only in the existing editor. Honor platform
            // replacement ranges as scalar ranges without creating a second buffer.
            let r = range
                .map(|r| self.range_from_utf16(r))
                .or(self.marked.clone())
                .unwrap_or_else(|| {
                    let n = self.editor.command.chars().count();
                    n..n
                });
            let start = self
                .editor
                .command
                .char_indices()
                .nth(r.start)
                .map_or(self.editor.command.len(), |p| p.0);
            let end = self
                .editor
                .command
                .char_indices()
                .nth(r.end)
                .map_or(self.editor.command.len(), |p| p.0);
            let clean: String = text.chars().filter(|c| !c.is_control()).collect();
            self.editor.command.replace_range(start..end, &clean);
            self.marked = (composing && !clean.is_empty())
                .then_some(r.start..r.start + clean.chars().count());
            return;
        }
        let r = range
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let r = r.start.min(r.end)..r.end.max(r.start);
        let original = text;
        let text = if text.contains(['\r', '\n']) {
            Cow::Owned(self.editor.buffer.normalize_paste(text))
        } else {
            Cow::Borrowed(text)
        };
        if !composing && self.editor.replace_snippet(r.clone(), &text) {
            self.editor.dashboard = false;
            self.ime_selection = None;
            return;
        }
        if composing {
            self.editor.snippet = None;
        }
        self.editor.buffer.replace(r.clone(), &text);
        self.editor.dashboard = false;
        let end = r.start + text.chars().count();
        self.marked = (composing && !text.is_empty()).then_some(r.start..end);
        self.ime_selection = if self.marked.is_some() {
            let local = ropey::Rope::from_str(original);
            let converted = |at: usize| {
                let at = local.utf16_cu_to_char(at.min(local.len_utf16_cu()));
                if matches!(text, Cow::Borrowed(_)) {
                    at
                } else {
                    self.editor
                        .buffer
                        .normalize_paste(&local.slice(..at).to_string())
                        .chars()
                        .count()
                }
            };
            selected.map(|s| r.start + converted(s.start)..r.start + converted(s.end.max(s.start)))
        } else {
            None
        };
        self.editor.buffer.cursor = self.ime_selection.as_ref().map_or(end, |r| r.end);
        if !composing {
            self.editor.buffer.snap_cursor();
        }
    }
    pub fn unmark(&mut self) {
        self.marked = None;
        self.ime_selection = None;
        self.editor.buffer.snap_cursor();
    }
    pub fn set_buffer(&mut self, buffer: Buffer) {
        self.marked = None;
        self.ime_selection = None;
        self.keymap.clear();
        self.editor.set_buffer(buffer);
    }
    pub fn act(&mut self, action: Action) -> Option<Request> {
        self.unmark();
        self.editor.act(action)
    }
}
