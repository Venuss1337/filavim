//! Stage 2 orchestration. Input updates state immediately; workers return tagged results.
use crate::{
    buffer::Buffer,
    editor::{Mode, Request},
    input::Action,
    navigation::{
        Item, Match, NavigationWorkers, Preview, PreviewJob, ProjectIndex, SearchJob, Target,
        TreeJob, TreeRow,
    },
    storage::{History, IoEvent, IoJob, IoWorker, Mutation},
    workspace::{Axis, BufferId, ViewId, Workspace, absolute, display_path},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Grep,
    Recent,
    Buffers,
    Palette,
    Projects,
}
impl PickerKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Files => "FIND FILES",
            Self::Grep => "LIVE GREP",
            Self::Recent => "RECENT FILES",
            Self::Buffers => "BUFFERS",
            Self::Palette => "COMMAND PALETTE",
            Self::Projects => "RECENT PROJECTS",
        }
    }
}
pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub selected: usize,
    pub results: Vec<Match>,
    pub preview: Preview,
    pub busy: bool,
    pub message: String,
    pub changed: Option<Instant>,
    pub top: usize,
}
#[derive(Default)]
pub struct Explorer {
    pub visible: bool,
    pub focused: bool,
    pub rows: Vec<TreeRow>,
    pub selected: usize,
    pub top: usize,
    pub expanded: BTreeSet<PathBuf>,
    pub busy: bool,
    pub reveal: Option<PathBuf>,
}
#[derive(Clone)]
pub enum DialogKind {
    Open,
    Create { parent: PathBuf, directory: bool },
    Rename(PathBuf),
    Delete(PathBuf),
    Directory,
    Close(BufferId),
}
pub struct Dialog {
    pub title: String,
    pub prompt: String,
    pub input: String,
    pub kind: DialogKind,
}
pub enum Effect {
    Clipboard(String),
}
pub struct App {
    pub ide: crate::ide::Ide,
    pub workspace: Workspace,
    pub picker: Option<Picker>,
    pub explorer: Explorer,
    pub dialog: Option<Dialog>,
    pub history: History,
    pub index: Arc<ProjectIndex>,
    pub indexing: bool,
    pub effects: Vec<Effect>,
    workers: NavigationWorkers,
    io: IoWorker,
    open_serial: u64,
    opens: BTreeMap<ViewId, u64>,
    pub saves: BTreeSet<BufferId>,
    saving_paths: BTreeMap<BufferId, PathBuf>,
    quit_views: BTreeMap<BufferId, ViewId>,
    history_pending: usize,
    history_path: PathBuf,
    pub mutation_pending: bool,
    pub exit_requested: bool,
}
impl App {
    pub fn new(workspace: Workspace, history: History) -> Self {
        Self::with_history_path(workspace, history, History::path())
    }
    pub fn with_history_path(
        workspace: Workspace,
        mut history: History,
        history_path: PathBuf,
    ) -> Self {
        history.project(workspace.root.clone());
        for e in workspace.buffers.values() {
            if let Some(path) = &e.buffer.path {
                history.file(path.clone());
            }
        }
        if let Some(path) = &workspace.editor().buffer.path {
            history.file(path.clone());
        }
        let mut app = Self {
            ide: crate::ide::Ide::default(),
            workspace,
            picker: None,
            explorer: Explorer::default(),
            dialog: None,
            history,
            index: Arc::new(ProjectIndex::default()),
            indexing: false,
            effects: Vec::new(),
            workers: NavigationWorkers::default(),
            io: IoWorker::default(),
            open_serial: 0,
            opens: BTreeMap::new(),
            saves: BTreeSet::new(),
            saving_paths: BTreeMap::new(),
            quit_views: BTreeMap::new(),
            history_pending: 0,
            history_path,
            mutation_pending: false,
            exit_requested: false,
        };
        app.refresh();
        app.persist();
        app
    }
    fn send(&mut self, job: IoJob) {
        if self.io.sender.send(job).is_err() {
            self.notify("I/O worker stopped", true);
        }
    }
    fn persist(&mut self) {
        self.history_pending += 1;
        self.send(IoJob::History {
            history: self.history.clone(),
            path: self.history_path.clone(),
        });
    }
    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.workspace.editor_mut().notify(text, error);
    }
    pub fn mode(&self) -> Mode {
        if self.ide.prompt.is_some() {
            Mode::Dialog
        } else if self
            .ide
            .panel
            .as_ref()
            .is_some_and(|p| p.kind != crate::ide::PanelKind::Completion)
        {
            Mode::Picker
        } else if self.ide.terminal_focus {
            Mode::Terminal
        } else if self.dialog.is_some() {
            Mode::Dialog
        } else if self.picker.is_some() {
            Mode::Picker
        } else if self.explorer.focused {
            Mode::Explorer
        } else {
            self.workspace.editor().mode
        }
    }
    pub fn ready_to_exit(&self) -> bool {
        self.exit_requested
            && self.ide.busy == 0
            && self.saves.is_empty()
            && self.history_pending == 0
            && !self.mutation_pending
    }
    pub fn pending(&self) -> bool {
        self.indexing
            || !self.saves.is_empty()
            || !self.opens.is_empty()
            || self.history_pending > 0
            || self.mutation_pending
            || self.explorer.busy
            || self
                .picker
                .as_ref()
                .is_some_and(|p| p.busy || p.changed.is_some())
    }
    pub fn refresh(&mut self) {
        self.indexing = true;
        self.workers.index.submit((
            self.workspace.root.clone(),
            self.workspace.config.navigation.clone(),
        ));
        self.notify("Indexing workspace…", false);
    }
    fn request_tree(&mut self) {
        if self.explorer.visible {
            self.explorer.busy = true;
            self.workers.tree.submit(TreeJob {
                index: Arc::clone(&self.index),
                expanded: self.explorer.expanded.clone(),
            });
        }
    }
    fn source(&self, kind: PickerKind) -> Arc<Vec<Item>> {
        match kind {
            PickerKind::Files | PickerKind::Grep => Arc::clone(&self.index.files),
            PickerKind::Recent => Arc::new(
                self.history
                    .files
                    .iter()
                    .map(|p| Item::file(p.clone(), self.label(p)))
                    .collect(),
            ),
            PickerKind::Buffers => Arc::new(
                self.workspace
                    .buffers
                    .iter()
                    .rev()
                    .map(|(id, e)| Item {
                        label: format!(
                            "{}{}",
                            e.buffer
                                .path
                                .as_ref()
                                .map_or_else(|| format!("Untitled #{id}"), |p| self.label(p)),
                            if e.buffer.dirty() { " [+]" } else { "" }
                        ),
                        target: Target::Buffer(*id),
                        line: 0,
                        column: 0,
                        detail: "Open buffer — unsaved contents shown in preview".into(),
                    })
                    .collect(),
            ),
            PickerKind::Palette => Arc::new(palette()),
            PickerKind::Projects => Arc::new(
                self.history
                    .projects
                    .iter()
                    .map(|p| Item {
                        label: display_path(p),
                        target: Target::Project(p.clone()),
                        line: 0,
                        column: 0,
                        detail: "Enter to switch workspace. Open buffers remain available.".into(),
                    })
                    .collect(),
            ),
        }
    }
    pub fn label(&self, path: &Path) -> String {
        display_path(path.strip_prefix(&self.workspace.root).unwrap_or(path)).replace('\\', "/")
    }
    pub fn open_picker(&mut self, kind: PickerKind) {
        self.workspace.editor_mut().set_mode(Mode::Normal);
        self.workspace.stash();
        self.explorer.focused = false;
        self.picker = Some(Picker {
            kind,
            query: String::new(),
            selected: 0,
            results: Vec::new(),
            preview: Preview::default(),
            busy: true,
            message: String::new(),
            changed: None,
            top: 0,
        });
        self.request_search();
    }
    fn request_search(&mut self) {
        let Some(picker) = &self.picker else {
            return;
        };
        let (kind, query) = (picker.kind, picker.query.clone());
        let config = self.workspace.config.navigation.clone();
        let job = if kind == PickerKind::Grep {
            SearchJob::Grep {
                index: Arc::clone(&self.index),
                query,
                config,
            }
        } else {
            SearchJob::Fuzzy {
                items: self.source(kind),
                query,
                limit: config.max_results,
            }
        };
        self.workers.search.submit(job);
        if let Some(picker) = &mut self.picker {
            picker.busy = true;
            picker.changed = None;
        }
    }
    fn request_preview(&mut self) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        self.workers.preview.cancel();
        picker.preview = Preview::default();
        let Some(item) = picker.results.get(picker.selected).map(|m| m.item.clone()) else {
            return;
        };
        let text = match &item.target {
            Target::Buffer(id) => self
                .workspace
                .buffers
                .get(id)
                .map(|e| e.buffer.text.clone()),
            Target::File(path) => self
                .workspace
                .find_path(path)
                .map(|id| self.workspace.buffers[&id].buffer.text.clone()),
            _ => None,
        };
        let query = if picker.kind == PickerKind::Grep {
            picker.query.clone()
        } else {
            String::new()
        };
        self.workers.preview.submit(PreviewJob {
            item,
            query,
            text,
            max_bytes: self.workspace.config.navigation.preview_max_bytes,
        });
    }
    fn query_changed(&mut self) {
        self.workers.search.cancel();
        self.workers.preview.cancel();
        if let Some(p) = &mut self.picker {
            p.selected = 0;
            p.top = 0;
            p.changed = Some(Instant::now());
            p.busy = true;
            p.preview = Preview::default();
            p.results.clear();
        }
    }
    pub fn type_text(&mut self, text: &str) {
        let mut ide = std::mem::take(&mut self.ide);
        let handled = ide.type_text(self, text);
        self.ide = ide;
        if handled {
            return;
        }
        if let Some(dialog) = &mut self.dialog {
            dialog
                .input
                .extend(text.chars().filter(|c| !c.is_control()));
        } else if let Some(p) = &mut self.picker {
            p.query.extend(text.chars().filter(|c| !c.is_control()));
            self.query_changed();
        } else if !self.explorer.focused {
            self.workspace.type_text(text);
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.picker.is_some() || self.dialog.is_some() {
            self.type_text(text);
        } else if !self.explorer.focused {
            self.workspace.paste(text);
        }
    }
    pub fn act(&mut self, action: Action) {
        let mut ide = std::mem::take(&mut self.ide);
        let handled = ide.action(self, action);
        self.ide = ide;
        if handled {
            return;
        }
        if self.dialog.is_some() {
            self.dialog_action(action);
            return;
        }
        if self.picker.is_some() {
            self.picker_action(action);
            return;
        }
        use Action::*;
        if self.explorer.focused {
            match action {
                Normal | Explorer => {
                    self.explorer.focused = false;
                    if action == Explorer {
                        self.explorer.visible = false;
                    }
                }
                Up => self.explorer.selected = self.explorer.selected.saturating_sub(1),
                Down => {
                    self.explorer.selected =
                        (self.explorer.selected + 1).min(self.explorer.rows.len().saturating_sub(1))
                }
                PageUp => self.explorer.selected = self.explorer.selected.saturating_sub(10),
                PageDown => {
                    self.explorer.selected = (self.explorer.selected + 10)
                        .min(self.explorer.rows.len().saturating_sub(1))
                }
                Submit => self.explorer_open(),
                ParentDirectory => self.explorer_parent(),
                CreateFile => self.file_dialog(false),
                CreateDirectory => self.file_dialog(true),
                RenameFile => self.rename_dialog(),
                DeleteFile => self.delete_dialog(),
                Refresh => self.refresh(),
                FindFiles => self.open_picker(PickerKind::Files),
                Palette => self.open_picker(PickerKind::Palette),
                _ => {}
            }
            return;
        }
        if self.workspace.editor().help {
            self.workspace.apply(action);
            return;
        }
        if self.workspace.editor().mode == Mode::Command && action == Submit {
            let command = std::mem::take(&mut self.workspace.editor_mut().command);
            let searching = self.workspace.editor().searching;
            self.workspace.editor_mut().set_mode(Mode::Normal);
            if searching {
                self.workspace.editor_mut().last_search = command;
                let req = self.workspace.apply(SearchNext);
                self.handle_request(req);
            } else {
                self.command(&command);
            }
            return;
        }
        if self.workspace.editor().mode == Mode::Command {
            let req = self.workspace.apply(action);
            self.handle_request(req);
            return;
        }
        match action {
            FindFiles => self.open_picker(PickerKind::Files),
            LiveGrep => self.open_picker(PickerKind::Grep),
            RecentFiles => self.open_picker(PickerKind::Recent),
            Buffers => self.open_picker(PickerKind::Buffers),
            Palette => self.open_picker(PickerKind::Palette),
            Projects => self.open_picker(PickerKind::Projects),
            Explorer => {
                self.explorer.visible = !self.explorer.visible;
                self.explorer.focused = self.explorer.visible;
                self.request_tree();
            }
            SplitVertical => {
                self.workspace.split(Axis::Vertical);
                self.explorer.focused = false;
            }
            SplitHorizontal => {
                self.workspace.split(Axis::Horizontal);
                self.explorer.focused = false;
            }
            FocusNext => self.workspace.focus_next(false),
            FocusPrevious => self.workspace.focus_next(true),
            GrowSplit => self.workspace.resize(5),
            ShrinkSplit => self.workspace.resize(-5),
            CloseView | Quit => self.close_or_quit(false),
            CloseBuffer => self.close_buffer(false),
            New => {
                self.opens.remove(&self.workspace.focused());
                self.workspace.add_buffer(Buffer::default());
            }
            NextBuffer => self.cycle_buffer(false),
            PreviousBuffer => self.cycle_buffer(true),
            NewTab => self.workspace.new_tab(),
            NextTab => self.workspace.switch_tab(self.workspace.active_tab + 1),
            PreviousTab => self.workspace.switch_tab(
                (self.workspace.active_tab + self.workspace.tabs.len() - 1)
                    % self.workspace.tabs.len(),
            ),
            _ => {
                let req = self.workspace.apply(action);
                self.handle_request(req);
            }
        }
    }
    fn picker_action(&mut self, action: Action) {
        use Action::*;
        match action {
            Normal => {
                self.picker = None;
                self.workers.search.cancel();
                self.workers.preview.cancel();
            }
            Up | Down | PageUp | PageDown => {
                let p = self.picker.as_mut().unwrap();
                let step = if matches!(action, PageUp | PageDown) {
                    10
                } else {
                    1
                };
                p.selected = if matches!(action, Up | PageUp) {
                    p.selected.saturating_sub(step)
                } else {
                    (p.selected + step).min(p.results.len().saturating_sub(1))
                };
                self.request_preview();
            }
            Backspace => {
                pop_grapheme(&mut self.picker.as_mut().unwrap().query);
                self.query_changed();
            }
            Refresh => self.refresh(),
            Submit => {
                let p = self.picker.as_ref().unwrap();
                let grep = p.kind == PickerKind::Grep;
                if let Some(item) = p.results.get(p.selected).map(|m| m.item.clone()) {
                    self.picker = None;
                    self.workers.search.cancel();
                    self.workers.preview.cancel();
                    match item.target {
                        Target::File(path) => self.open(
                            path,
                            if grep { item.line } else { usize::MAX },
                            item.column,
                            None,
                        ),
                        Target::Buffer(id) => self.workspace.switch_buffer(id),
                        Target::Action(action) => self.act(action),
                        Target::Project(path) => self.send(IoJob::Root(path)),
                        Target::Command(command) => self.command(&command),
                    }
                }
            }
            _ => {}
        }
    }
    fn cycle_buffer(&mut self, back: bool) {
        let ids: Vec<_> = self.workspace.buffers.keys().copied().collect();
        let at = ids
            .iter()
            .position(|id| *id == self.workspace.active_buffer())
            .unwrap();
        let next = if back {
            (at + ids.len() - 1) % ids.len()
        } else {
            (at + 1) % ids.len()
        };
        self.workspace.switch_buffer(ids[next]);
    }
    fn close_buffer(&mut self, force: bool) {
        let id = self.workspace.active_buffer();
        if self.saves.contains(&id) {
            self.notify("Wait for this buffer's save before closing it", true);
            return;
        }
        if !force && self.workspace.editor().buffer.dirty() {
            self.dialog = Some(Dialog {
                title: "CLOSE MODIFIED BUFFER".into(),
                prompt: "Type discard to close without saving. Esc cancels.".into(),
                input: String::new(),
                kind: DialogKind::Close(id),
            });
            return;
        }
        if let Err(e) = self.workspace.close_buffer(id, force) {
            self.notify(e, true);
        }
    }
    fn close_or_quit(&mut self, force: bool) {
        if self.workspace.close_view() {
            return;
        }
        if !self.saves.is_empty() {
            self.notify("Save in progress. Wait before quitting.", true);
            return;
        }
        if self.mutation_pending {
            self.notify("File operation in progress. Wait before quitting.", true);
            return;
        }
        if self.workspace.dirty() && !force {
            self.notify(
                "Unsaved buffers remain. Save them, :bd to close, or :qa! to discard all.",
                true,
            );
            return;
        }
        self.exit_requested = true;
    }
    pub fn open(
        &mut self,
        path: PathBuf,
        line: usize,
        column: usize,
        reload: Option<(BufferId, u64)>,
    ) {
        let path = absolute(&self.workspace.root, &path);
        let view = self.workspace.focused();
        if reload.is_none()
            && let Some(id) = self.workspace.find_path(&path)
        {
            self.workspace.switch_buffer(id);
            if line != usize::MAX {
                self.jump(id, view, line, column);
            }
            self.history.file(path);
            self.persist();
            return;
        }
        self.open_serial += 1;
        self.opens.insert(view, self.open_serial);
        self.send(IoJob::Open {
            serial: self.open_serial,
            view,
            path,
            line,
            column,
            reload,
        });
        self.notify("Opening…", false);
    }
    fn jump(&mut self, id: BufferId, view: ViewId, line: usize, column: usize) {
        let e = self.workspace.buffers.get_mut(&id).unwrap();
        let row = line.min(e.buffer.text.len_lines() - 1);
        let start = e.buffer.text.line_to_char(row);
        let cursor = (start + column).min(e.buffer.line_end(row));
        if let Some(v) = self.workspace.views.get_mut(&view) {
            v.cursor = cursor;
            v.top = row.saturating_sub(5);
            v.mode = Mode::Normal;
            v.anchor = None;
        }
        if self.workspace.focused() == view {
            self.workspace.load(view);
        }
    }
    fn handle_request(&mut self, request: Option<Request>) {
        match request {
            Some(Request::Clipboard(text)) => self.effects.push(Effect::Clipboard(text)),
            Some(Request::Save {
                path,
                text,
                state,
                quit,
            }) => {
                let id = self.workspace.active_buffer();
                let path = absolute(&self.workspace.root, &path);
                if self.mutation_pending {
                    self.notify("Wait for the file operation before saving", true);
                    return;
                }
                if self.saves.contains(&id) {
                    self.notify("A save is already in progress for this buffer", true);
                    return;
                }
                if self
                    .workspace
                    .find_path(&path)
                    .is_some_and(|other| other != id)
                {
                    self.notify("Destination belongs to another open buffer", true);
                    return;
                }
                if self.saving_paths.values().any(|p| *p == path) {
                    self.notify("Another save already targets this destination", true);
                    return;
                }
                let mut protected: Vec<_> = self
                    .workspace
                    .buffers
                    .iter()
                    .filter(|(other, _)| **other != id)
                    .filter_map(|(_, e)| e.buffer.path.clone())
                    .collect();
                protected.extend(self.saving_paths.values().cloned());
                self.saving_paths.insert(id, path.clone());
                self.saves.insert(id);
                if quit {
                    self.quit_views.insert(id, self.workspace.focused());
                }
                self.send(IoJob::Save {
                    buffer: id,
                    path,
                    text,
                    state,
                    quit,
                    protected,
                });
            }
            Some(Request::Search {
                text,
                query,
                start,
                revision,
            }) => self.send(IoJob::Search {
                buffer: self.workspace.active_buffer(),
                revision,
                text,
                query,
                start,
            }),
            Some(Request::Open { path, .. }) => self.open(path, usize::MAX, 0, None),
            Some(Request::Quit) => self.close_or_quit(false),
            None => {}
        }
    }
    pub fn command(&mut self, command: &str) {
        let command = command.trim().trim_start_matches(':');
        let (name, arg) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let arg = unquote(arg.trim());
        let mut ide = std::mem::take(&mut self.ide);
        let handled = ide.command(self, name, arg);
        self.ide = ide;
        if handled {
            return;
        }
        match name {
            "open" => {
                self.dialog = Some(Dialog {
                    title: "OPEN FILE".into(),
                    prompt: "File path (relative to workspace)".into(),
                    input: String::new(),
                    kind: DialogKind::Open,
                })
            }
            "e" | "edit" | "e!" => {
                let path = if arg.is_empty() {
                    self.workspace.editor().buffer.path.clone()
                } else {
                    Some(PathBuf::from(arg))
                };
                if let Some(path) = path {
                    let path = absolute(&self.workspace.root, &path);
                    let reload = if name == "e!" || arg.is_empty() {
                        self.workspace
                            .find_path(&path)
                            .map(|id| (id, self.workspace.buffers[&id].buffer.revision))
                    } else {
                        None
                    };
                    if name != "e!"
                        && reload.is_some_and(|(id, _)| self.workspace.buffers[&id].buffer.dirty())
                    {
                        self.notify("Unsaved changes. Use :e! to reload and discard.", true);
                    } else {
                        self.open(path, usize::MAX, 0, reload);
                    }
                } else {
                    self.notify("Use :e <path>", true);
                }
            }
            "new" | "enew" => {
                self.opens.remove(&self.workspace.focused());
                self.workspace.add_buffer(Buffer::default());
            }
            "new!" => {
                let id = self.workspace.active_buffer();
                if self.saves.contains(&id) {
                    self.notify(
                        "Wait for the pending save before discarding this buffer",
                        true,
                    );
                    return;
                }
                self.opens.remove(&self.workspace.focused());
                let single = self.workspace.buffers.len() == 1;
                if let Err(e) = self.workspace.close_buffer(id, true) {
                    self.notify(e, true);
                } else if !single {
                    self.workspace.add_buffer(Buffer::default());
                }
            }
            "bd" | "bdelete" => self.close_buffer(false),
            "bd!" => self.close_buffer(true),
            "bn" | "bnext" => self.cycle_buffer(false),
            "bp" | "bprevious" => self.cycle_buffer(true),
            "buffers" | "ls" => self.open_picker(PickerKind::Buffers),
            "q" | "quit" => self.close_or_quit(false),
            "q!" => self.close_or_quit(true),
            "qa" | "qa!" => {
                if !self.saves.is_empty() || self.mutation_pending {
                    self.notify("Wait for pending file operations", true);
                } else if self.workspace.dirty() && name != "qa!" {
                    self.notify("Unsaved buffers remain. Use :qa! to discard all.", true);
                } else {
                    self.exit_requested = true;
                }
            }
            "vsplit" | "split" => {
                self.workspace.split(if name == "vsplit" {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                });
                if !arg.is_empty() {
                    self.open(arg.into(), usize::MAX, 0, None);
                }
            }
            "tabnew" => {
                self.workspace.new_tab();
                if !arg.is_empty() {
                    self.open(arg.into(), usize::MAX, 0, None);
                }
            }
            "tabnext" => self.workspace.switch_tab(self.workspace.active_tab + 1),
            "tabprevious" => self.workspace.switch_tab(
                (self.workspace.active_tab + self.workspace.tabs.len() - 1)
                    % self.workspace.tabs.len(),
            ),
            "cd" | "lcd" => {
                if arg.is_empty() {
                    self.dialog = Some(Dialog {
                        title: "SWITCH WORKSPACE".into(),
                        prompt: "Directory path (relative to current workspace)".into(),
                        input: String::new(),
                        kind: DialogKind::Directory,
                    });
                } else {
                    self.send(IoJob::Root(absolute(&self.workspace.root, Path::new(arg))));
                }
            }
            "pwd" => self.notify(self.workspace.root.display().to_string(), false),
            "files" => self.open_picker(PickerKind::Files),
            "grep" => {
                self.open_picker(PickerKind::Grep);
                self.type_text(arg);
            }
            "recent" => self.open_picker(PickerKind::Recent),
            "projects" => self.open_picker(PickerKind::Projects),
            "commands" => self.open_picker(PickerKind::Palette),
            "explore" => self.act(Action::Explorer),
            "refresh" => self.refresh(),
            _ => {
                let req = self.workspace.editor_mut().execute_command(command);
                self.handle_request(req);
                self.workspace.stash();
            }
        }
    }
    fn explorer_open(&mut self) {
        let Some(row) = self.explorer.rows.get(self.explorer.selected).cloned() else {
            return;
        };
        if row.entry.directory {
            if !self.explorer.expanded.remove(&row.entry.relative) {
                self.explorer.expanded.insert(row.entry.relative);
            }
            self.request_tree();
        } else {
            self.explorer.focused = false;
            self.open(row.entry.path, usize::MAX, 0, None);
        }
    }
    fn explorer_parent(&mut self) {
        let Some(row) = self.explorer.rows.get(self.explorer.selected) else {
            return;
        };
        let path = if row.expanded {
            row.entry.relative.clone()
        } else {
            row.entry
                .relative
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf()
        };
        self.explorer.expanded.remove(&path);
        if let Some(i) = self
            .explorer
            .rows
            .iter()
            .position(|r| r.entry.relative == path)
        {
            self.explorer.selected = i;
        }
        self.request_tree();
    }
    fn selected_parent(&self) -> PathBuf {
        self.explorer.rows.get(self.explorer.selected).map_or_else(
            || self.workspace.root.clone(),
            |row| {
                if row.entry.directory {
                    row.entry.path.clone()
                } else {
                    row.entry
                        .path
                        .parent()
                        .unwrap_or(&self.workspace.root)
                        .to_path_buf()
                }
            },
        )
    }
    fn file_dialog(&mut self, directory: bool) {
        let parent = self.selected_parent();
        self.dialog = Some(Dialog {
            title: if directory {
                "CREATE DIRECTORY"
            } else {
                "CREATE FILE"
            }
            .into(),
            prompt: format!("Name inside {}", self.label(&parent)),
            input: String::new(),
            kind: DialogKind::Create { parent, directory },
        });
    }
    fn rename_dialog(&mut self) {
        if let Some(row) = self.explorer.rows.get(self.explorer.selected) {
            self.dialog = Some(Dialog {
                title: "RENAME".into(),
                prompt: format!(
                    "Rename {} (name or relative sibling path)",
                    self.label(&row.entry.path)
                ),
                input: row
                    .entry
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                kind: DialogKind::Rename(row.entry.path.clone()),
            });
        }
    }
    fn delete_dialog(&mut self) {
        if let Some(row) = self.explorer.rows.get(self.explorer.selected) {
            if self.workspace.buffers.values().any(|e| {
                e.buffer.dirty()
                    && e.buffer
                        .path
                        .as_ref()
                        .is_some_and(|p| p.starts_with(&row.entry.path))
            }) {
                self.notify(
                    "Save or close modified buffers under this path before deleting it",
                    true,
                );
                return;
            }
            self.dialog = Some(Dialog {
                title: "DELETE FROM DISK".into(),
                prompt: format!(
                    "{} — type delete to permanently remove it and its contents",
                    self.label(&row.entry.path)
                ),
                input: String::new(),
                kind: DialogKind::Delete(row.entry.path.clone()),
            });
        }
    }
    fn dialog_action(&mut self, action: Action) {
        match action {
            Action::Normal => self.dialog = None,
            Action::ClearInput => self.dialog.as_mut().unwrap().input.clear(),
            Action::Backspace => pop_grapheme(&mut self.dialog.as_mut().unwrap().input),
            Action::Submit => {
                let d = self.dialog.as_ref().unwrap();
                let input = d.input.trim();
                let kind = d.kind.clone();
                if input.is_empty() {
                    return;
                }
                let operation = match kind {
                    DialogKind::Open => {
                        let path = PathBuf::from(input);
                        self.dialog = None;
                        self.open(path, usize::MAX, 0, None);
                        return;
                    }
                    DialogKind::Close(id) => {
                        if input != "discard" {
                            self.notify("Type discard exactly, or Esc to cancel", true);
                            return;
                        }
                        self.dialog = None;
                        if self.saves.contains(&id) {
                            self.notify("Wait for the pending save", true);
                            return;
                        }
                        if let Err(e) = self.workspace.close_buffer(id, true) {
                            self.notify(e, true);
                        }
                        return;
                    }
                    DialogKind::Directory => {
                        let path = absolute(&self.workspace.root, Path::new(input));
                        self.dialog = None;
                        self.send(IoJob::Root(path));
                        return;
                    }
                    DialogKind::Create { parent, directory } => Mutation::Create {
                        path: absolute(&parent, Path::new(input)),
                        directory,
                    },
                    DialogKind::Rename(from) => Mutation::Rename {
                        to: absolute(from.parent().unwrap(), Path::new(input)),
                        from,
                    },
                    DialogKind::Delete(path) => {
                        if input != "delete" {
                            self.notify("Type delete exactly, or Esc to cancel", true);
                            return;
                        }
                        if self.workspace.buffers.values().any(|e| {
                            e.buffer.dirty()
                                && e.buffer.path.as_ref().is_some_and(|p| p.starts_with(&path))
                        }) {
                            self.notify(
                                "A modified buffer now uses this path. Save it before deleting.",
                                true,
                            );
                            return;
                        }
                        Mutation::Delete { path }
                    }
                };
                if self.mutation_pending || !self.saves.is_empty() {
                    self.notify(
                        "Wait for pending file operations before changing the filesystem",
                        true,
                    );
                    return;
                }
                self.dialog = None;
                self.mutation_pending = true;
                self.send(IoJob::Mutation {
                    root: self.workspace.root.clone(),
                    operation,
                });
            }
            _ => {}
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut budget = crate::background::DrainBudget::new(16);
        while budget.allow() {
            let Ok(event) = self.io.results.try_recv() else {
                break;
            };
            changed = true;
            self.io_event(event);
        }
        if let Some(index) = self.workers.index.poll() {
            self.index = index;
            self.indexing = false;
            changed = true;
            self.notify(
                format!(
                    "{} files indexed{} · {} inaccessible entries",
                    self.index.files.len(),
                    if self.index.truncated {
                        " (limit reached)"
                    } else {
                        ""
                    },
                    self.index.skipped
                ),
                false,
            );
            self.request_tree();
            if self
                .picker
                .as_ref()
                .is_some_and(|p| matches!(p.kind, PickerKind::Files | PickerKind::Grep))
            {
                self.request_search();
            }
        }
        if let Some(rows) = self.workers.tree.poll() {
            let selected = self.explorer.reveal.take().or_else(|| {
                self.explorer
                    .rows
                    .get(self.explorer.selected)
                    .map(|r| r.entry.path.clone())
            });
            self.explorer.rows = rows;
            self.explorer.busy = false;
            self.explorer.selected = selected
                .and_then(|p| self.explorer.rows.iter().position(|r| r.entry.path == p))
                .unwrap_or(
                    self.explorer
                        .selected
                        .min(self.explorer.rows.len().saturating_sub(1)),
                );
            changed = true;
        }
        if let Some(result) = self.workers.search.poll()
            && let Some(p) = &mut self.picker
        {
            p.results = result.matches;
            p.message = result.message;
            p.busy = false;
            p.selected = p.selected.min(p.results.len().saturating_sub(1));
            self.request_preview();
            changed = true;
        }
        if let Some(preview) = self.workers.preview.poll()
            && let Some(p) = &mut self.picker
        {
            p.preview = preview;
            changed = true;
        }
        if self
            .picker
            .as_ref()
            .and_then(|p| p.changed)
            .is_some_and(|at| {
                at.elapsed() >= Duration::from_millis(self.workspace.config.navigation.debounce_ms)
            })
        {
            self.request_search();
            changed = true;
        }
        let mut ide = std::mem::take(&mut self.ide);
        changed |= ide.tick(self);
        self.ide = ide;
        changed
    }
    fn io_event(&mut self, event: IoEvent) {
        match event {
            IoEvent::Superseded => {}
            IoEvent::Open {
                serial,
                view,
                path,
                line,
                column,
                reload,
                result,
            } => {
                if self.opens.get(&view) != Some(&serial) {
                    return;
                }
                self.opens.remove(&view);
                if !self.workspace.views.contains_key(&view) {
                    return;
                }
                match result {
                    Err(e) => self.notify(format!("Open failed: {e:#}"), true),
                    Ok(text) => {
                        let id = if let Some((id, revision)) = reload {
                            if self.saves.contains(&id)
                                || self
                                    .workspace
                                    .buffers
                                    .get(&id)
                                    .is_none_or(|e| e.buffer.revision != revision)
                            {
                                self.notify("Reload cancelled: buffer changed or is saving", true);
                                return;
                            }
                            self.workspace
                                .buffers
                                .get_mut(&id)
                                .unwrap()
                                .set_buffer(Buffer::new(text, Some(path.clone())));
                            for v in self.workspace.views.values_mut().filter(|v| v.buffer == id) {
                                v.cursor = v
                                    .cursor
                                    .min(self.workspace.buffers[&id].buffer.text.len_chars());
                                v.mode = Mode::Normal;
                                v.anchor = None;
                            }
                            self.workspace.load(self.workspace.focused());
                            id
                        } else if let Some(id) = self.workspace.find_path(&path) {
                            id
                        } else {
                            self.workspace
                                .insert_buffer(Buffer::new(text, Some(path.clone())))
                        };
                        if self.workspace.focused() == view {
                            self.workspace.switch_buffer(id);
                        } else if let Some(v) = self.workspace.views.get_mut(&view) {
                            v.buffer = id;
                            v.cursor = 0;
                            v.top = 0;
                            v.left = 0;
                            v.mode = Mode::Normal;
                            v.anchor = None;
                        }
                        if line != usize::MAX {
                            self.jump(id, view, line, column);
                        }
                        self.history.file(path);
                        self.persist();
                        self.notify(
                            "File opened. Other buffers and unsaved edits are preserved.",
                            false,
                        );
                    }
                }
            }
            IoEvent::Save {
                buffer,
                path,
                state,
                quit,
                result,
                created,
            } => {
                self.saves.remove(&buffer);
                self.saving_paths.remove(&buffer);
                let quit_view = self.quit_views.remove(&buffer);
                match result {
                    Err(e) => self.notify(format!("Save failed: {e:#}"), true),
                    Ok(()) => {
                        let mut reindex = created;
                        if let Some(e) = self.workspace.buffers.get_mut(&buffer) {
                            reindex |= e.buffer.path.as_ref() != Some(&path);
                            e.buffer.path = Some(path.clone());
                            e.buffer.mark_saved(state);
                        }
                        if reindex && path.starts_with(&self.workspace.root) {
                            self.refresh();
                        }
                        self.history.file(path);
                        self.persist();
                        if quit
                            && self.workspace.active_buffer() == buffer
                            && quit_view == Some(self.workspace.focused())
                            && !self.workspace.editor().buffer.dirty()
                        {
                            self.close_or_quit(false);
                        } else {
                            let dirty = self
                                .workspace
                                .buffers
                                .get(&buffer)
                                .is_some_and(|e| e.buffer.dirty());
                            self.notify(
                                if dirty {
                                    "Snapshot saved; newer edits remain unsaved."
                                } else {
                                    "Saved."
                                },
                                false,
                            );
                        }
                    }
                }
            }
            IoEvent::Search {
                buffer,
                revision,
                result,
            } => {
                if self.workspace.active_buffer() == buffer
                    && self.workspace.editor().buffer.revision == revision
                {
                    if let Some(pos) = result {
                        self.workspace.editor_mut().buffer.cursor = pos;
                        self.workspace.editor_mut().buffer.snap_cursor();
                        self.workspace.stash();
                        self.notify("Match found. N finds the next result.", false);
                    } else {
                        self.notify("Pattern not found", true);
                    }
                }
            }
            IoEvent::Root(result) => match result {
                Err(e) => self.notify(format!("{e:#}"), true),
                Ok(path) => {
                    self.workspace.root = path.clone();
                    self.history.project(path);
                    self.persist();
                    self.index = Arc::new(ProjectIndex::default());
                    self.explorer.rows.clear();
                    self.explorer.expanded.clear();
                    self.explorer.selected = 0;
                    self.workers.tree.cancel();
                    self.workers.search.cancel();
                    self.workers.preview.cancel();
                    self.refresh();
                }
            },
            IoEvent::Mutation { operation, result } => {
                self.mutation_pending = false;
                match result {
                    Err(e) => self.notify(format!("File operation failed: {e:#}"), true),
                    Ok(()) => {
                        let mut retained_edits = false;
                        match operation {
                            Mutation::Rename { from, to } => {
                                for e in self.workspace.buffers.values_mut() {
                                    if let Some(path) = &e.buffer.path
                                        && let Ok(suffix) = path.strip_prefix(&from)
                                    {
                                        e.buffer.path = Some(if suffix.as_os_str().is_empty() {
                                            to.clone()
                                        } else {
                                            to.join(suffix)
                                        });
                                    }
                                }
                                for path in &mut self.history.files {
                                    if let Ok(suffix) = path.strip_prefix(&from) {
                                        *path = if suffix.as_os_str().is_empty() {
                                            to.clone()
                                        } else {
                                            to.join(suffix)
                                        };
                                    }
                                }
                                if let (Ok(from_relative), Ok(to_relative)) = (
                                    from.strip_prefix(&self.workspace.root),
                                    to.strip_prefix(&self.workspace.root),
                                ) {
                                    self.explorer.expanded = self
                                        .explorer
                                        .expanded
                                        .iter()
                                        .map(|p| {
                                            p.strip_prefix(from_relative).map_or_else(
                                                |_| p.clone(),
                                                |suffix| {
                                                    if suffix.as_os_str().is_empty() {
                                                        to_relative.to_path_buf()
                                                    } else {
                                                        to_relative.join(suffix)
                                                    }
                                                },
                                            )
                                        })
                                        .collect();
                                }
                                self.reveal(&to);
                                self.persist();
                            }
                            Mutation::Delete { path } => {
                                let ids: Vec<_> = self
                                    .workspace
                                    .buffers
                                    .iter()
                                    .filter(|(_, e)| {
                                        e.buffer.path.as_ref().is_some_and(|p| p.starts_with(&path))
                                    })
                                    .map(|(id, _)| *id)
                                    .collect();
                                for id in ids {
                                    if self.workspace.buffers[&id].buffer.dirty() {
                                        self.workspace.buffers.get_mut(&id).unwrap().buffer.path =
                                            None;
                                        retained_edits = true;
                                    } else {
                                        let _ = self.workspace.close_buffer(id, true);
                                    }
                                }
                                self.history.files.retain(|p| !p.starts_with(&path));
                                self.persist();
                            }
                            Mutation::Create { path, directory } => {
                                self.reveal(&path);
                                if !directory {
                                    self.explorer.focused = false;
                                    self.open(path, usize::MAX, 0, None);
                                }
                            }
                        }
                        self.refresh();
                        self.notify(
                            if retained_edits {
                                "Deleted from disk; newer edits were preserved in an unnamed buffer"
                            } else {
                                "File operation completed"
                            },
                            false,
                        );
                    }
                }
            }
            IoEvent::History(result) => {
                self.history_pending = self.history_pending.saturating_sub(1);
                if let Err(e) = result {
                    self.notify(format!("Recent history could not be saved: {e:#}"), true);
                }
            }
        }
    }
    fn reveal(&mut self, path: &Path) {
        self.explorer.reveal = Some(path.to_path_buf());
        if let Ok(relative) = path.strip_prefix(&self.workspace.root) {
            for parent in relative
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                self.explorer.expanded.insert(parent.to_path_buf());
            }
        }
    }
}
fn unquote(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
pub(crate) fn pop_grapheme(text: &mut String) {
    let end = text
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i);
    text.truncate(end);
}
pub fn palette() -> Vec<Item> {
    use Action::*;
    let mut items: Vec<Item> = [
        ("Settings", Settings, ":Settings / Space ,"),
        ("Setup and installations", Setup, ":Setup"),
        ("LSP information", LspInfo, "Space l"),
        ("Hover documentation", Hover, "Shift-K"),
        ("Go to definition", Definition, "Space g d"),
        ("Go to implementation", Implementation, "Space g i"),
        ("Find references", References, "Space g r"),
        ("Rename symbol", RenameSymbol, "Space c r"),
        ("Code actions", CodeActions, "Space c a"),
        ("Format document/selection", FormatDocument, "Space c f"),
        ("Git status", GitStatus, "Space g s"),
        ("Git diff/hunks", GitDiff, "Space g h"),
        ("Stage active file", GitStage, ":GitStage"),
        ("Unstage active file", GitUnstage, ":GitUnstage"),
        ("Toggle terminal", Terminal, "Space t"),
        ("Snippets", Snippets, "Space c s"),
        (
            "Find files",
            FindFiles,
            "Space f f · Fuzzy search indexed workspace files",
        ),
        (
            "Live grep",
            LiveGrep,
            "Space f g · Smart-case regular expressions across the project",
        ),
        (
            "Recent files",
            RecentFiles,
            "Space f r · Reopen previously accessed files",
        ),
        (
            "Recent projects",
            Projects,
            "Space f p · Switch to a recent workspace; :cd enters a path",
        ),
        (
            "Buffers",
            Buffers,
            "Space b b · Switch buffers without losing unsaved changes",
        ),
        (
            "Toggle file explorer",
            Explorer,
            "Space e · WASD navigation; I create, R rename, X delete",
        ),
        (
            "Split vertically",
            SplitVertical,
            "Space s v · Shared document, independent view",
        ),
        (
            "Split horizontally",
            SplitHorizontal,
            "Space s h · Shared document, independent view",
        ),
        ("Focus next split", FocusNext, "Space s n"),
        ("Focus previous split", FocusPrevious, "Space s p"),
        ("Grow split", GrowSplit, "Space s +"),
        ("Shrink split", ShrinkSplit, "Space s -"),
        (
            "Close split or tab",
            CloseView,
            "Space s x · Last view exits only when all buffers are saved",
        ),
        ("New tab", NewTab, "Space b t n"),
        ("Next tab", NextTab, "Space b t d"),
        ("Previous tab", PreviousTab, "Space b t a"),
        ("New buffer", New, "Space n · Existing edits remain open"),
        (
            "Close buffer",
            CloseBuffer,
            "Space b x · Modified buffers require confirmation",
        ),
        ("Next buffer", NextBuffer, "Space b n"),
        ("Previous buffer", PreviousBuffer, "Space b p"),
        (
            "Save file",
            Save,
            "Space w / Ctrl-S · Use :w <path> to name an untitled buffer",
        ),
        (
            "Command line",
            Command,
            ": · Commands include :wq, :e, :cd, :split, :vsplit, :tabnew, :bd, :qa",
        ),
        ("Undo", Undo, "U"),
        ("Redo", Redo, "Ctrl-R"),
        ("Copy", Copy, "Y · Selection or entire line"),
        ("Cut", Cut, "C · Selection or entire line"),
        ("Paste", Paste, "P · Internal register"),
        ("Help", Help, "F1 / :help"),
        ("Quit", Quit, ":q · Preserves unsaved buffers"),
    ]
    .into_iter()
    .map(|(label, action, detail)| Item {
        label: label.into(),
        target: Target::Action(action),
        line: 0,
        column: 0,
        detail: detail.into(),
    })
    .collect();
    for (label, command, detail) in [
        (
            "Open file (:e)",
            "open",
            "Enter a file path in a dialog; existing buffers are preserved",
        ),
        (
            "Change workspace (:cd)",
            "cd",
            "Enter a directory path; open buffers remain available",
        ),
        (
            "Save and close (:wq)",
            "wq",
            "Save the active buffer before closing its view",
        ),
        (
            "Refresh project index (:refresh)",
            "refresh",
            "Rescan files, including changes made outside Filavim",
        ),
        (
            "Show workspace directory (:pwd)",
            "pwd",
            "Display the full workspace path",
        ),
        (
            "Quit all (:qa)",
            "qa",
            "Exit only when every buffer is saved",
        ),
    ] {
        items.push(Item {
            label: label.into(),
            target: Target::Command(command.into()),
            line: 0,
            column: 0,
            detail: detail.into(),
        });
    }
    items
}
