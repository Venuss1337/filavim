//! Shared documents with independent views, recursive split layouts, and tab pages.
use crate::{
    buffer::Buffer,
    config::Config,
    editor::{Editor, Mode, Request},
    input::Action,
};
use ratatui::layout::Rect;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

pub type BufferId = u64;
pub type ViewId = u64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}
#[derive(Clone, Debug)]
pub enum Split {
    Leaf(ViewId),
    Branch {
        axis: Axis,
        ratio: u16,
        first: Box<Split>,
        second: Box<Split>,
    },
}
impl Split {
    fn split(&mut self, target: ViewId, new: ViewId, axis: Axis) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Branch {
                    axis,
                    ratio: 50,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new)),
                };
                true
            }
            Self::Branch { first, second, .. } => {
                first.split(target, new, axis) || second.split(target, new, axis)
            }
            _ => false,
        }
    }
    fn remove(self, target: ViewId) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Branch {
                axis,
                ratio,
                first,
                second,
            } => match (first.remove(target), second.remove(target)) {
                (Some(first), Some(second)) => Some(Self::Branch {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (a, b) => a.or(b),
            },
        }
    }
    pub fn leaves(&self, out: &mut Vec<ViewId>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Branch { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }
    pub fn layout(&self, area: Rect, out: &mut Vec<(ViewId, Rect)>) {
        match self {
            Self::Leaf(id) => out.push((*id, area)),
            Self::Branch {
                axis,
                ratio,
                first,
                second,
            } => {
                let total = if *axis == Axis::Vertical {
                    area.width
                } else {
                    area.height
                };
                let size = ((u32::from(total) * u32::from(*ratio)) / 100) as u16;
                let a = if *axis == Axis::Vertical {
                    Rect::new(area.x, area.y, size, area.height)
                } else {
                    Rect::new(area.x, area.y, area.width, size)
                };
                let b = if *axis == Axis::Vertical {
                    Rect::new(area.x + size, area.y, total - size, area.height)
                } else {
                    Rect::new(area.x, area.y + size, area.width, total - size)
                };
                first.layout(a, out);
                second.layout(b, out);
            }
        }
    }
    fn resize(&mut self, target: ViewId, delta: i16) -> bool {
        if let Self::Branch {
            ratio,
            first,
            second,
            ..
        } = self
        {
            if first.resize(target, delta) || second.resize(target, delta) {
                return true;
            }
            let mut leaves = Vec::new();
            first.leaves(&mut leaves);
            let sign = if leaves.contains(&target) { 1 } else { -1 };
            leaves.clear();
            second.leaves(&mut leaves);
            if sign == 1 || leaves.contains(&target) {
                *ratio = (*ratio as i16 + sign * delta).clamp(15, 85) as u16;
                return true;
            }
        }
        false
    }
}
#[derive(Clone, Debug)]
pub struct View {
    pub buffer: BufferId,
    pub cursor: usize,
    pub top: usize,
    pub left: usize,
    pub mode: Mode,
    pub anchor: Option<usize>,
    pub page_height: usize,
    preferred_column: Option<usize>,
}
impl View {
    fn new(buffer: BufferId) -> Self {
        Self {
            buffer,
            cursor: 0,
            top: 0,
            left: 0,
            mode: Mode::Normal,
            anchor: None,
            page_height: 20,
            preferred_column: None,
        }
    }
}
pub struct Tab {
    pub layout: Split,
    pub focused: ViewId,
}
pub struct Workspace {
    pub buffers: BTreeMap<BufferId, Editor>,
    pub views: BTreeMap<ViewId, View>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub root: PathBuf,
    pub config: Config,
    next_buffer: u64,
    next_view: u64,
    register: String,
    register_linewise: bool,
}
impl Workspace {
    pub fn new(editor: Editor, root: PathBuf) -> Self {
        let config = editor.config.clone();
        let cursor = editor.buffer.cursor;
        let mut view = View::new(1);
        view.cursor = cursor;
        Self {
            buffers: BTreeMap::from([(1, editor)]),
            views: BTreeMap::from([(1, view)]),
            tabs: vec![Tab {
                layout: Split::Leaf(1),
                focused: 1,
            }],
            active_tab: 0,
            root,
            config,
            next_buffer: 2,
            next_view: 2,
            register: String::new(),
            register_linewise: false,
        }
    }
    pub fn focused(&self) -> ViewId {
        self.tabs[self.active_tab].focused
    }
    pub fn active_buffer(&self) -> BufferId {
        self.views[&self.focused()].buffer
    }
    pub fn editor(&self) -> &Editor {
        &self.buffers[&self.active_buffer()]
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        let id = self.active_buffer();
        self.buffers.get_mut(&id).unwrap()
    }
    pub fn stash(&mut self) {
        self.capture(self.focused());
    }
    pub fn capture(&mut self, focused: ViewId) {
        let editor = &self.buffers[&self.views[&focused].buffer];
        let state = (
            editor.buffer.cursor,
            editor.top,
            editor.left,
            editor.mode,
            editor.anchor,
            editor.page_height,
        );
        let view = self.views.get_mut(&focused).unwrap();
        (
            view.cursor,
            view.top,
            view.left,
            view.mode,
            view.anchor,
            view.page_height,
        ) = state;
        view.preferred_column = editor.buffer.preferred_column;
    }
    pub fn load(&mut self, view: ViewId) {
        let view = self.views[&view].clone();
        let editor = self.buffers.get_mut(&view.buffer).unwrap();
        editor.buffer.cursor = view.cursor.min(editor.buffer.text.len_chars());
        editor.buffer.snap_cursor();
        editor.buffer.preferred_column = view.preferred_column;
        editor.top = view.top;
        editor.left = view.left;
        editor.mode = view.mode;
        editor.anchor = view.anchor.map(|a| a.min(editor.buffer.text.len_chars()));
        editor.page_height = view.page_height;
    }
    pub fn switch_view(&mut self, view: ViewId) {
        self.stash();
        self.editor_mut().buffer.end_group();
        self.tabs[self.active_tab].focused = view;
        self.load(view);
        self.editor_mut().notify("", false);
        if self.editor().mode == Mode::Insert {
            self.editor_mut().buffer.begin_group();
        }
    }
    pub fn focus_next(&mut self, back: bool) {
        let mut ids = Vec::new();
        self.tabs[self.active_tab].layout.leaves(&mut ids);
        let index = ids.iter().position(|id| *id == self.focused()).unwrap();
        let next = if back {
            (index + ids.len() - 1) % ids.len()
        } else {
            (index + 1) % ids.len()
        };
        self.switch_view(ids[next]);
    }
    pub fn split(&mut self, axis: Axis) {
        self.stash();
        let old = self.focused();
        let id = self.next_view;
        self.next_view += 1;
        let mut view = self.views[&old].clone();
        view.mode = Mode::Normal;
        view.anchor = None;
        self.views.insert(id, view);
        self.tabs[self.active_tab].layout.split(old, id, axis);
        self.switch_view(id);
    }
    pub fn resize(&mut self, delta: i16) {
        let id = self.focused();
        self.tabs[self.active_tab].layout.resize(id, delta);
    }
    pub fn new_tab(&mut self) {
        self.stash();
        let id = self.next_view;
        self.next_view += 1;
        let mut view = self.views[&self.focused()].clone();
        view.mode = Mode::Normal;
        view.anchor = None;
        self.views.insert(id, view);
        self.tabs.push(Tab {
            layout: Split::Leaf(id),
            focused: id,
        });
        self.switch_tab(self.tabs.len() - 1);
    }
    pub fn switch_tab(&mut self, index: usize) {
        self.stash();
        self.editor_mut().buffer.end_group();
        self.active_tab = index % self.tabs.len();
        self.load(self.focused());
        self.editor_mut().notify("", false);
        if self.editor().mode == Mode::Insert {
            self.editor_mut().buffer.begin_group();
        }
    }
    pub fn switch_buffer(&mut self, id: BufferId) {
        if !self.buffers.contains_key(&id) || self.active_buffer() == id {
            return;
        }
        self.stash();
        self.editor_mut().buffer.end_group();
        let view = self.focused();
        let prior = self
            .views
            .values()
            .find(|v| v.buffer == id)
            .cloned()
            .unwrap_or_else(|| {
                let mut v = View::new(id);
                v.cursor = self.buffers[&id].buffer.cursor;
                v.top = self.buffers[&id].top;
                v.left = self.buffers[&id].left;
                v
            });
        self.views.insert(
            view,
            View {
                mode: Mode::Normal,
                anchor: None,
                ..prior
            },
        );
        self.load(view);
        self.editor_mut().notify("", false);
    }
    pub fn add_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = self.insert_buffer(buffer);
        self.switch_buffer(id);
        id
    }
    pub fn insert_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = self.next_buffer;
        self.next_buffer += 1;
        let mut editor = Editor::new(self.config.clone());
        editor.set_buffer(buffer);
        self.buffers.insert(id, editor);
        id
    }
    pub fn find_path(&self, path: &Path) -> Option<BufferId> {
        self.buffers
            .iter()
            .find(|(_, e)| e.buffer.path.as_deref() == Some(path))
            .map(|(id, _)| *id)
    }
    pub fn dirty(&self) -> bool {
        self.buffers.values().any(|e| e.buffer.dirty())
    }
    pub fn close_buffer(&mut self, id: BufferId, force: bool) -> Result<(), String> {
        if self.buffers.get(&id).is_some_and(|e| e.buffer.dirty()) && !force {
            return Err("Unsaved buffer. Save it or use :bd! to discard.".into());
        }
        if !self.buffers.contains_key(&id) {
            return Ok(());
        }
        self.stash();
        let replacement = self
            .buffers
            .keys()
            .copied()
            .find(|other| *other != id)
            .unwrap_or_else(|| {
                let next = self.next_buffer;
                self.next_buffer += 1;
                let mut e = Editor::new(self.config.clone());
                e.dashboard = false;
                self.buffers.insert(next, e);
                next
            });
        for view in self.views.values_mut() {
            if view.buffer == id {
                *view = View::new(replacement);
            }
        }
        self.buffers.remove(&id);
        self.load(self.focused());
        Ok(())
    }
    pub fn close_view(&mut self) -> bool {
        self.stash();
        self.editor_mut().buffer.end_group();
        let id = self.focused();
        let layout = self.tabs[self.active_tab].layout.clone().remove(id);
        if let Some(layout) = layout {
            let mut ids = Vec::new();
            layout.leaves(&mut ids);
            self.tabs[self.active_tab].layout = layout;
            self.tabs[self.active_tab].focused = ids[0];
            self.views.remove(&id);
            self.load(ids[0]);
            true
        } else if self.tabs.len() > 1 {
            self.tabs.remove(self.active_tab);
            self.views.remove(&id);
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
            self.load(self.focused());
            true
        } else {
            if self.editor().mode == Mode::Insert {
                self.editor_mut().buffer.begin_group();
            }
            false
        }
    }
    pub fn apply(&mut self, action: Action) -> Option<Request> {
        if action == Action::Paste {
            let register = self.register.clone();
            let linewise = self.register_linewise;
            self.editor_mut().register = register;
            self.editor_mut().register_linewise = linewise;
        }
        let revision = self.editor().buffer.revision;
        let register_changed = self.editor().selection().is_some()
            || self.editor().mode != Mode::Insert
            || matches!(action, Action::Copy | Action::Cut | Action::DeleteLine);
        let request = self.editor_mut().act(action);
        if register_changed
            && matches!(
                action,
                Action::Copy
                    | Action::Cut
                    | Action::Delete
                    | Action::DeleteLine
                    | Action::Backspace
            )
        {
            self.register = self.editor().register.clone();
            self.register_linewise = self.editor().register_linewise;
        }
        self.after_edit(revision);
        request
    }
    pub fn type_text(&mut self, text: &str) {
        let revision = self.editor().buffer.revision;
        self.editor_mut().type_text(text);
        self.after_edit(revision);
    }
    pub fn paste(&mut self, text: &str) {
        let revision = self.editor().buffer.revision;
        self.editor_mut().paste_external(text);
        self.after_edit(revision);
    }
    fn after_edit(&mut self, previous: u64) {
        let id = self.active_buffer();
        let focused = self.focused();
        let b = &self.editor().buffer;
        if b.revision != previous {
            let len = b.text.len_chars();
            let change = b.last_change;
            for (view_id, v) in &mut self.views {
                if v.buffer == id && *view_id != focused {
                    v.cursor = adjust_position(v.cursor, change, len);
                    v.anchor = v.anchor.map(|p| adjust_position(p, change, len));
                    v.preferred_column = None;
                }
            }
        }
        self.stash();
    }
    /// Apply validated external edits as one undo group, adjusting every view.
    pub fn external_edits(&mut self, id: BufferId, edits: &[(std::ops::Range<usize>, String)]) {
        self.stash();
        let focused = self.focused();
        let active = self.active_buffer();
        let editor = self.buffers.get_mut(&id).unwrap();
        editor.buffer.end_group();
        editor.buffer.begin_group();
        for (range, text) in edits {
            let text = editor.buffer.normalize_paste(text);
            editor.buffer.replace(range.clone(), &text);
            let change = editor.buffer.last_change;
            let len = editor.buffer.text.len_chars();
            for (view_id, view) in &mut self.views {
                if view.buffer == id {
                    view.cursor = if id == active && *view_id == focused {
                        editor.buffer.cursor
                    } else {
                        adjust_position(view.cursor, change, len)
                    };
                    view.anchor = view.anchor.map(|p| adjust_position(p, change, len));
                    view.preferred_column = None;
                }
            }
        }
        editor.buffer.end_group();
        self.load(focused);
        if self.editor().mode == Mode::Insert {
            self.editor_mut().buffer.begin_group();
        }
    }
    pub fn live_buffer_ids(&self) -> BTreeSet<BufferId> {
        self.views.values().map(|v| v.buffer).collect()
    }
}
fn adjust_position(p: usize, change: Option<(usize, usize, usize)>, len: usize) -> usize {
    match change {
        Some((_, old_end, new_end)) if p >= old_end => p
            .saturating_add_signed(new_end as isize - old_end as isize)
            .min(len),
        Some((start, old_end, new_end)) if p >= start && p < old_end => new_end.min(len),
        _ => p.min(len),
    }
}

/// Normalize without touching disk. Existing paths are canonicalized on the I/O worker.
pub fn absolute(root: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
pub fn project_root(start: &Path) -> PathBuf {
    find_project_root(start).unwrap_or_else(|| {
        if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        }
    })
}
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    for dir in start.ancestors() {
        if [
            ".git",
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "go.mod",
            ".filavim",
        ]
        .iter()
        .any(|marker| dir.join(marker).exists())
        {
            return Some(dir.to_path_buf());
        }
    }
    None
}
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    }
}
