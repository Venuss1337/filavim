//! Terminal-independent text, grapheme navigation, and persistent undo snapshots.
use ropey::{Rope, RopeSlice};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::VecDeque,
    ops::Range,
    path::PathBuf,
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

#[derive(Clone)]
struct Snapshot {
    text: Rope,
    cursor: usize,
    state: u64,
}

#[derive(Default)]
struct DisplayLine {
    row: usize,
    tabs: usize,
    // Relative character positions, always at grapheme boundaries.
    points: Vec<(usize, usize)>,
}

/// A bounded edit journal shared by protocol sync, parsing, and completion rebasing.
/// Coordinates describe the document immediately before this edit.
#[derive(Clone, Debug)]
pub struct TextChange {
    pub revision: u64,
    pub range: Range<usize>,
    pub text: std::sync::Arc<str>,
    pub ranges: [lsp_types::Range; 3],
    pub syntax: tree_sitter::InputEdit,
}

pub struct Buffer {
    pub text: Rope,
    /// Cursor is a Unicode scalar index, always on a grapheme boundary.
    pub cursor: usize,
    pub path: Option<PathBuf>,
    pub newline: &'static str,
    pub revision: u64,
    pub last_change: Option<(usize, usize, usize)>,
    state: u64,
    saved_state: u64,
    next_state: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    grouped: bool,
    checkpointed: bool,
    pub(crate) preferred_column: Option<usize>,
    changes: VecDeque<TextChange>,
    change_bytes: usize,
    column_cache: Cell<Option<(u64, usize, usize, usize)>>,
    display_lines: RefCell<VecDeque<DisplayLine>>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new(Rope::new(), None)
    }
}

impl Buffer {
    pub fn new(text: Rope, path: Option<PathBuf>) -> Self {
        let newline = text
            .lines()
            .take(1000)
            .find_map(|l| {
                let mut chars = l.chars_at(l.len_chars());
                if chars.prev() == Some('\n') {
                    if chars.prev() == Some('\r') {
                        Some("\r\n")
                    } else {
                        Some("\n")
                    }
                } else {
                    None
                }
            })
            .unwrap_or("\n");
        Self {
            text,
            cursor: 0,
            path,
            newline,
            revision: 0,
            last_change: None,
            state: 0,
            saved_state: 0,
            next_state: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            grouped: false,
            checkpointed: false,
            preferred_column: None,
            changes: VecDeque::new(),
            change_bytes: 0,
            column_cache: Cell::new(None),
            display_lines: RefCell::new(VecDeque::new()),
        }
    }

    pub fn dirty(&self) -> bool {
        self.state != self.saved_state
    }
    pub fn state(&self) -> u64 {
        self.state
    }
    pub fn mark_saved(&mut self, state: u64) {
        self.saved_state = state;
    }
    pub fn row(&self) -> usize {
        self.text.char_to_line(self.cursor)
    }
    pub fn line(&self, row: usize) -> String {
        self.line_slice(row).to_string()
    }
    pub fn line_slice(&self, row: usize) -> RopeSlice<'_> {
        let line = self.text.line(row);
        let mut end = line.len_chars();
        while end > 0 && matches!(line.char(end - 1), '\r' | '\n') {
            end -= 1;
        }
        line.slice(..end)
    }
    pub fn line_end(&self, row: usize) -> usize {
        self.text.line_to_char(row) + self.line_slice(row).len_chars()
    }
    pub fn column(&self, tab_width: usize) -> usize {
        if let Some((revision, cursor, tabs, column)) = self.column_cache.get()
            && (revision, cursor, tabs) == (self.revision, self.cursor, tab_width)
        {
            return column;
        }
        let start = self.text.line_to_char(self.row());
        let column = self
            .display_position(self.row(), self.cursor - start, usize::MAX, tab_width)
            .1;
        self.column_cache
            .set(Some((self.revision, self.cursor, tab_width, column)));
        column
    }

    /// Locate a display column without revisiting an already indexed long prefix.
    pub fn column_at(&self, cursor: usize, tabs: usize) -> usize {
        let row = self.text.char_to_line(cursor);
        self.display_position(row, cursor - self.text.line_to_char(row), usize::MAX, tabs)
            .1
    }
    pub fn display_start(&self, row: usize, column: usize, tabs: usize) -> (usize, usize) {
        if column == 0 {
            return (0, 0);
        }
        self.display_position(row, usize::MAX, column, tabs)
    }

    fn display_position(
        &self,
        row: usize,
        character: usize,
        column: usize,
        tabs: usize,
    ) -> (usize, usize) {
        let text = self.line_slice(row);
        if text.len_chars() <= 1024 {
            let (mut ch, mut col) = (0, 0);
            for g in rope_graphemes(text) {
                let next = ch + g.chars().count();
                let width = grapheme_width(&g, col, tabs);
                if next > character || col + width > column {
                    break;
                }
                ch = next;
                col += width;
            }
            return (ch, col);
        }
        let mut lines = self.display_lines.borrow_mut();
        let index = lines.iter().position(|l| l.row == row && l.tabs == tabs);
        let mut line = index
            .and_then(|i| lines.remove(i))
            .unwrap_or_else(|| DisplayLine {
                row,
                tabs,
                points: vec![(0, 0)],
            });
        let index = line
            .points
            .partition_point(|&(ch, col)| ch <= character && col <= column)
            .saturating_sub(1);
        let (mut ch, mut col) = line.points[index];
        let text = self.line_slice(row);
        for g in rope_graphemes(text.slice(ch..)) {
            let width = grapheme_width(&g, col, tabs);
            let next = ch + g.chars().count();
            if next > character || col + width > column {
                break;
            }
            ch = next;
            col += width;
            if ch >= line.points.last().unwrap().0 + 1024 {
                line.points.push((ch, col));
            }
        }
        // At most eight recently used lines, about 16 KiB of checkpoints per
        // million scalar values. No document-wide eager index or invalidation.
        lines.push_front(line);
        lines.truncate(8);
        (ch, col)
    }
    pub fn recent_changes(&self) -> Vec<TextChange> {
        self.changes.iter().cloned().collect()
    }
    pub fn changes_since(&self, revision: u64) -> Option<Vec<TextChange>> {
        if revision == self.revision {
            return Some(Vec::new());
        }
        if revision > self.revision || self.changes.front()?.revision > revision + 1 {
            return None;
        }
        Some(
            self.changes
                .iter()
                .filter(|e| e.revision > revision)
                .cloned()
                .collect(),
        )
    }
    pub fn begin_group(&mut self) {
        self.grouped = true;
        self.checkpointed = false;
    }
    pub fn end_group(&mut self) {
        self.grouped = false;
        self.checkpointed = false;
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            state: self.state,
        }
    }
    fn checkpoint(&mut self) {
        if !self.grouped || !self.checkpointed {
            if self.undo.len() >= 512 {
                self.undo.remove(0);
            }
            self.undo.push(self.snapshot());
            self.checkpointed = true;
        }
        self.redo.clear();
        self.state = self.next_state;
        self.next_state += 1;
        self.revision += 1;
        self.preferred_column = None;
    }
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        if range.is_empty() && text.is_empty() {
            return;
        }
        self.checkpoint();
        let start = range.start;
        let row = self.text.char_to_line(start);
        let safe = self
            .previous(start)
            .saturating_sub(self.text.line_to_char(row));
        self.display_lines.borrow_mut().retain_mut(|line| {
            if line.row > row {
                return false;
            }
            if line.row == row {
                line.points.retain(|&(ch, _)| ch == 0 || ch < safe);
            }
            true
        });
        let point = |at| {
            let row = self.text.char_to_line(at);
            tree_sitter::Point::new(
                row,
                self.text.char_to_byte(at) - self.text.line_to_byte(row),
            )
        };
        let mut edit = tree_sitter::InputEdit {
            start_byte: self.text.char_to_byte(start),
            old_end_byte: self.text.char_to_byte(range.end),
            new_end_byte: self.text.char_to_byte(start) + text.len(),
            start_position: point(start),
            old_end_position: point(range.end),
            new_end_position: point(start),
        };
        let ranges = [
            crate::lsp::Encoding::Utf8,
            crate::lsp::Encoding::Utf16,
            crate::lsp::Encoding::Utf32,
        ]
        .map(|encoding| crate::lsp::selection_range(&self.text, range.clone(), encoding));
        self.last_change = Some((start, range.end, start + text.chars().count()));
        self.text.remove(range);
        self.text.insert(start, text);
        self.cursor = start + text.chars().count();
        let row = self.text.char_to_line(self.cursor);
        edit.new_end_position = tree_sitter::Point::new(
            row,
            self.text.char_to_byte(self.cursor) - self.text.line_to_byte(row),
        );
        self.change_bytes += text.len();
        self.changes.push_back(TextChange {
            revision: self.revision,
            range: start..self.last_change.unwrap().1,
            text: text.into(),
            ranges,
            syntax: edit,
        });
        while self.changes.len() > 256 || self.change_bytes > 1024 * 1024 {
            if let Some(old) = self.changes.pop_front() {
                self.change_bytes -= old.text.len();
            }
        }
        // Joining text can form a new combining cluster across the insertion boundary.
        self.snap_cursor();
        #[cfg(feature = "perf")]
        crate::perf::trace(
            "edit_applied",
            format!(
                "{}@{}",
                self.path
                    .as_deref()
                    .map(crate::lsp::path_key)
                    .unwrap_or_default(),
                self.revision
            ),
        );
    }
    pub fn insert(&mut self, text: &str) {
        self.replace(self.cursor..self.cursor, text);
    }
    pub fn insert_paste(&mut self, text: &str) {
        self.insert(&self.normalize_paste(text));
    }
    pub fn normalize_paste(&self, text: &str) -> String {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized.replace('\n', self.newline)
    }
    pub fn newline(&mut self, auto_indent: bool) {
        let indent: String = if auto_indent {
            self.line_slice(self.row())
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect()
        } else {
            String::new()
        };
        self.insert(&format!("{}{indent}", self.newline));
    }
    pub fn previous(&self, pos: usize) -> usize {
        self.boundary(pos, false)
    }
    pub fn next(&self, pos: usize) -> usize {
        self.boundary(pos, true)
    }
    fn boundary(&self, pos: usize, forward: bool) -> usize {
        if !forward && pos == 0 {
            return 0;
        }
        if forward && pos >= self.text.len_chars() {
            return self.text.len_chars();
        }
        let byte = self.text.char_to_byte(pos);
        let mut cursor = GraphemeCursor::new(byte, self.text.len_bytes(), true);
        let (mut chunk, mut start, _, _) =
            self.text
                .chunk_at_byte(if forward { byte } else { byte - 1 });
        loop {
            let result = if forward {
                cursor.next_boundary(chunk, start)
            } else {
                cursor.prev_boundary(chunk, start)
            };
            match result {
                Ok(Some(byte)) => return self.text.byte_to_char(byte),
                Ok(None) => return if forward { self.text.len_chars() } else { 0 },
                Err(GraphemeIncomplete::NextChunk) => {
                    (chunk, start, _, _) = self.text.chunk_at_byte(start + chunk.len());
                }
                Err(GraphemeIncomplete::PrevChunk) => {
                    (chunk, start, _, _) = self.text.chunk_at_byte(start - 1);
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context, context_start, _, _) = self.text.chunk_at_byte(end - 1);
                    cursor.provide_context(&context[..end - context_start], context_start);
                }
                Err(GraphemeIncomplete::InvalidOffset) => {
                    unreachable!("rope chunks cover the grapheme cursor")
                }
            }
        }
    }
    pub fn snap_cursor(&mut self) {
        self.cursor = self.cursor.min(self.text.len_chars());
        if self.cursor > 0 {
            self.cursor = self.next(self.previous(self.cursor));
        }
    }
    pub fn horizontal(&mut self, right: bool) {
        self.cursor = if right {
            self.next(self.cursor)
        } else {
            self.previous(self.cursor)
        };
        self.preferred_column = None;
    }
    pub fn vertical(&mut self, delta: isize, tab_width: usize) {
        let target = self
            .preferred_column
            .unwrap_or_else(|| self.column(tab_width));
        self.preferred_column = Some(target);
        let row = self
            .row()
            .saturating_add_signed(delta)
            .min(self.text.len_lines() - 1);
        self.cursor = self.text.line_to_char(row) + self.display_start(row, target, tab_width).0;
    }
    pub fn home(&mut self) {
        self.cursor = self.text.line_to_char(self.row());
        self.preferred_column = None;
    }
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.row());
        self.preferred_column = None;
    }
    pub fn word(&mut self, forward: bool) {
        // Work on rope characters, never allocate or scan the whole document.
        if forward {
            let initial = self.text.get_char(self.cursor).map(word_class);
            while self.cursor < self.text.len_chars()
                && self.text.get_char(self.cursor).map(word_class) == initial
            {
                self.cursor = self.next(self.cursor);
            }
            while self.cursor < self.text.len_chars() && self.text.char(self.cursor).is_whitespace()
            {
                self.cursor = self.next(self.cursor);
            }
        } else {
            self.cursor = self.previous(self.cursor);
            while self.cursor > 0 && self.text.char(self.cursor).is_whitespace() {
                self.cursor = self.previous(self.cursor);
            }
            let class = self.text.get_char(self.cursor).map(word_class);
            while self.cursor > 0 {
                let prev = self.previous(self.cursor);
                if self.text.get_char(prev).map(word_class) != class {
                    break;
                }
                self.cursor = prev;
            }
        }
        self.preferred_column = None;
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.replace(self.previous(self.cursor)..self.cursor, "");
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.text.len_chars() {
            self.replace(self.cursor..self.next(self.cursor), "");
        }
    }
    pub fn undo(&mut self) -> bool {
        self.end_group();
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        self.end_group();
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
            true
        } else {
            false
        }
    }
    fn restore(&mut self, s: Snapshot) {
        self.display_lines.get_mut().clear();
        self.changes.clear();
        self.change_bytes = 0;
        self.last_change = None;
        self.text = s.text;
        self.cursor = s.cursor;
        self.state = s.state;
        self.revision += 1;
        self.preferred_column = None;
    }
}

/// Grapheme segmentation across rope chunks, allocating only for a cluster that
/// actually crosses a chunk boundary. Callers can stop at the viewport edge.
pub fn rope_graphemes(text: RopeSlice<'_>) -> impl Iterator<Item = Cow<'_, str>> {
    let mut at = 0;
    let mut contiguous = text.as_str().map(|s| s.graphemes(true));
    let (mut local, mut local_start) = if text.len_bytes() == 0 {
        ("", 0)
    } else {
        let (chunk, start, _, _) = text.chunk_at_byte(0);
        (chunk, start)
    };
    std::iter::from_fn(move || {
        if let Some(iter) = &mut contiguous {
            return iter.next().map(Cow::Borrowed);
        }
        if at >= text.len_bytes() {
            return None;
        }
        if at >= local_start + local.len() {
            let (chunk, start, _, _) = text.chunk_at_byte(at);
            local = chunk;
            local_start = start;
        }
        let g = local[at - local_start..].graphemes(true).next().unwrap();
        let end = at + g.len();
        // Interior clusters cannot cross a rope chunk. Only its last cluster
        // needs the context-aware slow path (flags, combining marks, ZWJ, CRLF).
        if end < local_start + local.len() || end == text.len_bytes() {
            at = end;
            return Some(Cow::Borrowed(g));
        }
        let mut cursor = GraphemeCursor::new(at, text.len_bytes(), true);
        let (mut chunk, mut start, _, _) = text.chunk_at_byte(at);
        loop {
            match cursor.next_boundary(chunk, start) {
                Ok(Some(end)) => {
                    let slice = text.byte_slice(at..end);
                    at = end;
                    return Some(Cow::from(slice));
                }
                Ok(None) => return None,
                Err(GraphemeIncomplete::NextChunk) => {
                    (chunk, start, _, _) = text.chunk_at_byte(start + chunk.len());
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context, offset, _, _) = text.chunk_at_byte(end - 1);
                    cursor.provide_context(&context[..end - offset], offset);
                }
                Err(other) => unreachable!("forward rope grapheme: {other:?}"),
            }
        }
    })
}

fn word_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}
pub fn grapheme_width(g: &str, column: usize, tab_width: usize) -> usize {
    if g == "\t" {
        tab_width - column % tab_width
    } else {
        UnicodeWidthStr::width(g).max(1)
    }
}
pub fn display_width(text: &str, tab_width: usize) -> usize {
    text.graphemes(true)
        .fold(0, |col, g| col + grapheme_width(g, col, tab_width))
}
