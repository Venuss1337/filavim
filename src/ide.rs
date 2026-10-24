//! Native IDE orchestration. Disk/process work is submitted to services; edits
//! are committed to shared buffers only after version checks and confirmation.
use crate::{
    app::App,
    editor::Mode,
    font, git,
    input::Action,
    lsp::{self, Encoding},
    setup, terminal,
    workspace::BufferId,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ops::Range,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    Completion,
    Locations,
    Actions,
    Text,
    Settings,
    Setup,
    Font,
    Git,
    Diff,
    Snippets,
}
#[derive(Clone)]
pub enum Choice {
    None,
    Completion(Value),
    Location {
        path: PathBuf,
        position: lsp_types::Position,
        encoding: Encoding,
    },
    CodeAction {
        server: String,
        value: Value,
    },
    Setting(String, String),
    Install(String),
    Font(font::Operation),
    FontSetup,
    Cancel,
    GitFile(PathBuf),
    Hunk(usize),
    Snippet(String),
}
#[derive(Clone)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub choice: Choice,
}
pub struct Panel {
    pub title: String,
    pub kind: PanelKind,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub query: String,
    pub text: String,
    pub scroll: usize,
    pub popup_size: (u16, u16),
    filter_cache: std::cell::RefCell<(String, usize, Vec<usize>)>,
    width_cache: std::cell::Cell<(usize, u16)>,
}
impl Panel {
    pub fn text(title: &str, text: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: PanelKind::Text,
            rows: Vec::new(),
            selected: 0,
            query: String::new(),
            text: text.into(),
            scroll: 0,
            popup_size: (0, 0),
            filter_cache: Default::default(),
            width_cache: std::cell::Cell::new((usize::MAX, 0)),
        }
    }
    pub fn list(title: &str, kind: PanelKind, rows: Vec<Row>) -> Self {
        Self {
            kind,
            rows,
            ..Self::text(title, "")
        }
    }
    pub fn invalidate(&self) {
        self.filter_cache.borrow_mut().1 = usize::MAX;
        self.width_cache.set((usize::MAX, 0));
    }
    pub fn completion_width(&self) -> u16 {
        if self.width_cache.get().0 != self.rows.len() {
            let width = self
                .rows
                .iter()
                .map(|r| crate::contextual_ui::label_width(&r.label).saturating_add(6))
                .max()
                .unwrap_or(0)
                .clamp(18, 52) as u16;
            self.width_cache.set((self.rows.len(), width));
        }
        self.width_cache.get().1
    }
    pub fn visible(&self) -> Vec<&Row> {
        let mut cache = self.filter_cache.borrow_mut();
        if cache.0 != self.query || cache.1 != self.rows.len() {
            let q = self.query.to_lowercase();
            cache.2 = self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    if q.is_empty() {
                        return Some(i);
                    }
                    let label = match &r.choice {
                        Choice::Completion(v) => v["filterText"].as_str().unwrap_or(&r.label),
                        _ => &r.label,
                    };
                    label.to_lowercase().contains(&q).then_some(i)
                })
                .collect();
            cache.0.clone_from(&self.query);
            cache.1 = self.rows.len();
        }
        cache.2.iter().map(|i| &self.rows[*i]).collect()
    }
}
pub enum PromptAction {
    Rename,
    Setting(String),
    Commit,
    Install(String),
    Font(font::Operation),
    Apply(Vec<lsp::PlannedDocument>),
    ApplyCommand {
        plan: Vec<lsp::PlannedDocument>,
        server: String,
        command: Value,
    },
    ServerApply {
        server: String,
        id: Value,
        plan: Vec<lsp::PlannedDocument>,
    },
}
pub struct Prompt {
    pub title: String,
    pub input: String,
    pub action: PromptAction,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Completion,
    Resolve,
    ResolvePreview,
    Hover,
    Definition,
    Implementation,
    References,
    Rename,
    Actions,
    ResolveAction,
    Format,
    RangeFormat,
    Signature,
    Execute,
}
struct Pending {
    server: String,
    buffer: BufferId,
    revision: u64,
    cursor: usize,
    kind: RequestKind,
    document: u64,
    mode: Mode,
}
struct CompletionJob {
    result: Value,
    needle: String,
    pending: Pending,
    server: String,
    encoding: Encoding,
    prefix: Range<usize>,
}
struct CompletionReady {
    rows: Vec<Row>,
    needle: String,
    pending: Pending,
    server: String,
    encoding: Encoding,
    prefix: Range<usize>,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreparationKey {
    Diagnostics(String, String),
    Response(String, u64, u8),
}
struct PreparationJob {
    server: String,
    epoch: u64,
    encoding: Encoding,
    value: Value,
    context: PreparationContext,
}
enum PreparationContext {
    Diagnostics {
        key: String,
        revision: u64,
        document: Option<(u64, u64)>,
    },
    Response {
        id: u64,
        pending: Pending,
    },
}
struct Prepared {
    server: String,
    epoch: u64,
    context: PreparationContext,
    content: PreparedContent,
}
enum PreparedContent {
    Diagnostics(Vec<Diagnostic>),
    Rows(Vec<Row>),
}
fn prepare(job: PreparationJob, cancel: &crate::background::Cancel) -> Prepared {
    let timer = crate::perf::Timer::start();
    let PreparationJob {
        server,
        epoch,
        encoding,
        mut value,
        context,
    } = job;
    let content = match &context {
        PreparationContext::Diagnostics { revision, .. } => {
            let values = value
                .get_mut("diagnostics")
                .and_then(Value::as_array_mut)
                .map(std::mem::take)
                .unwrap_or_default();
            let mut diagnostics = Vec::with_capacity(values.len());
            for (i, d) in values.into_iter().enumerate() {
                if i % 64 == 0 && cancel.cancelled() {
                    break;
                }
                if let (Ok(range), Some(message)) = (
                    serde_json::from_value(d["range"].clone()),
                    d["message"].as_str(),
                ) {
                    diagnostics.push(Diagnostic {
                        range,
                        severity: d["severity"].as_u64().unwrap_or(1) as u32,
                        message: message.into(),
                        encoding,
                        raw: d,
                        revision: *revision,
                    });
                }
            }
            diagnostics.sort_by_key(|d| d.range.start.line);
            PreparedContent::Diagnostics(diagnostics)
        }
        PreparationContext::Response { pending, .. } => {
            let values = if value.is_array() {
                std::mem::take(value.as_array_mut().unwrap())
            } else if value.is_object() {
                vec![value]
            } else {
                Vec::new()
            };
            let mut rows = Vec::with_capacity(values.len());
            for (i, v) in values.into_iter().enumerate() {
                if i % 64 == 0 && cancel.cancelled() {
                    break;
                }
                if pending.kind == RequestKind::Actions {
                    rows.push(Row {
                        label: v["title"].as_str().unwrap_or("Code action").into(),
                        detail: v["kind"].as_str().unwrap_or("").into(),
                        choice: Choice::CodeAction {
                            server: server.clone(),
                            value: v,
                        },
                    });
                } else if let Some(uri) = v["uri"].as_str().or(v["targetUri"].as_str())
                    && let Ok(path) = lsp::path(uri)
                    && let Some(pos) = v
                        .get("range")
                        .or(v.get("targetSelectionRange"))
                        .and_then(|r| r.get("start"))
                    && let Ok(position) = serde_json::from_value::<lsp_types::Position>(pos.clone())
                {
                    rows.push(Row {
                        label: format!(
                            "{}:{}:{}",
                            path.display(),
                            position.line + 1,
                            position.character + 1
                        ),
                        detail: String::new(),
                        choice: Choice::Location {
                            path,
                            position,
                            encoding,
                        },
                    });
                }
            }
            PreparedContent::Rows(rows)
        }
    };
    timer.finish("lsp_preparation");
    #[cfg(feature = "perf")]
    crate::perf::trace(
        "lsp_prepared",
        match &context {
            PreparationContext::Diagnostics { key, revision, .. } => {
                format!("{server}:{key}@{revision}")
            }
            PreparationContext::Response { id, .. } => format!("{server}:{id}"),
        },
    );
    Prepared {
        server,
        epoch,
        context,
        content,
    }
}
struct Server {
    client: lsp::Client,
    capabilities: Value,
    encoding: Encoding,
    status: String,
    documents: BTreeMap<BufferId, u64>,
    uris: BTreeMap<BufferId, String>,
    paths: BTreeMap<BufferId, PathBuf>,
    instances: BTreeMap<BufferId, u64>,
}
#[derive(Clone)]
pub struct Diagnostic {
    pub range: lsp_types::Range,
    pub severity: u32,
    pub message: String,
    pub encoding: Encoding,
    pub raw: Value,
    pub revision: u64,
}
pub enum Task {
    FontStatus {
        generation: u64,
        remove: bool,
        readonly: bool,
    },
    Font(font::Operation),
    Settings {
        path: PathBuf,
        key: String,
        value: String,
    },
    Discover,
    Install(String),
    Plan {
        edit: Value,
        snapshots: BTreeMap<String, lsp::Snapshot>,
        encoding: Encoding,
        source: Option<(String, Value)>,
        followup: Option<(String, Value)>,
    },
    Location {
        path: PathBuf,
        position: lsp_types::Position,
        encoding: Encoding,
    },
}
enum TaskResult {
    FontStatus {
        generation: u64,
        remove: bool,
        readonly: bool,
        report: Result<font::Report, String>,
    },
    Font(String),
    Settings(Box<crate::config::Config>),
    Discover(Vec<(String, String)>),
    Message(String),
    Plan(
        Result<Vec<lsp::PlannedDocument>, String>,
        Option<(String, Value)>,
        Option<(String, Value)>,
    ),
    Location(PathBuf, usize, usize),
}
struct Tasks {
    sender: mpsc::Sender<Task>,
    events: mpsc::Receiver<Result<TaskResult, String>>,
}
impl Default for Tasks {
    fn default() -> Self {
        let (sender, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(task) = rx.recv() {
                let r = (|| -> Result<TaskResult> {
                    match task {
                        Task::FontStatus {
                            generation,
                            remove,
                            readonly,
                        } => Ok(TaskResult::FontStatus {
                            generation,
                            remove,
                            readonly,
                            report: font::status().map_err(|e| format!("{e:#}")),
                        }),
                        Task::Font(operation) => Ok(TaskResult::Font(font::execute(operation)?)),
                        Task::Settings { path, key, value } => Ok(TaskResult::Settings(Box::new(
                            setup::save_setting(&path, &key, &value)?,
                        ))),
                        Task::Discover => Ok(TaskResult::Discover(setup::installed())),
                        Task::Install(id) => Ok(TaskResult::Message(setup::install(&id)?)),
                        Task::Plan {
                            edit,
                            snapshots,
                            encoding,
                            source,
                            followup,
                        } => Ok(TaskResult::Plan(
                            lsp::plan_edit(&edit, &snapshots, encoding).map_err(|e| e.to_string()),
                            source,
                            followup,
                        )),
                        Task::Location {
                            path,
                            position,
                            encoding,
                        } => {
                            let text = crate::files::open(&path)?;
                            let at = lsp::offset(&text, position, encoding)?;
                            let row = text.char_to_line(at);
                            Ok(TaskResult::Location(path, row, at - text.line_to_char(row)))
                        }
                    }
                })();
                if tx.send(r.map_err(|e| format!("{e:#}"))).is_err() {
                    break;
                }
                crate::background::wake();
            }
        });
        Self { sender, events }
    }
}
struct Snippet {
    buffer: BufferId,
    fields: Vec<Range<usize>>,
    index: usize,
    selected: bool,
    revision: u64,
}
#[derive(Default)]
pub struct Ide {
    pub signature: Option<crate::contextual::Signature>,
    pub panel: Option<Panel>,
    pub prompt: Option<Prompt>,
    pub diagnostics: BTreeMap<String, Vec<Diagnostic>>,
    pub git: git::State,
    pub terminals: Vec<terminal::Session>,
    pub terminal_visible: bool,
    pub terminal_focus: bool,
    pub terminal_vertical: bool,
    pub terminal_active: usize,
    pub config_epoch: u64,
    pub busy: usize,
    pub message: String,
    font_generation: u64,
    servers: BTreeMap<String, Server>,
    preparation: Option<crate::background::KeyedWorker<PreparationKey, PreparationJob, Prepared>>,
    preparation_epoch: u64,
    prepared_requests: BTreeMap<(String, u64, u8), u64>,
    server_turn: usize,
    pending: BTreeMap<u64, Pending>,
    serial: u64,
    root: Option<PathBuf>,
    changes: BTreeMap<BufferId, (u64, Instant)>,
    completion_at: Option<(BufferId, u64)>,
    completion_worker: Option<crate::background::LatestWorker<CompletionJob, CompletionReady>>,
    completion_context: Option<(BufferId, u64, usize, String, Encoding)>,
    completion_preview: Option<(BufferId, u64, usize)>,
    completion_accept: Option<(BufferId, u64, usize)>,
    completion_start: Option<usize>,
    completion_document: Option<u64>,
    completion_text: Option<ropey::Rope>,
    signature_seen: Option<(BufferId, u64, usize, u64, Mode)>,
    signature_due: Option<Instant>,
    tasks: Option<Tasks>,
    git_worker: Option<git::Service>,
    git_inflight: bool,
    git_stamp: Option<(BufferId, u64)>,
    git_requested: Option<Instant>,
    git_dirty: Option<Instant>,
    shutting_down: bool,
    git_error: Option<String>,
    snippet: Option<Snippet>,
    pub installations: Vec<(String, String)>,
}
impl Ide {
    fn invalidate_preparation(&mut self) {
        self.preparation_epoch += 1;
        self.prepared_requests.clear();
        if let Some(worker) = &mut self.preparation {
            worker.cancel_all();
        }
    }
    fn clear_panel(&mut self) {
        if let Some(panel) = self.panel.take()
            && panel.rows.len() > 200
        {
            self.retire_lsp(panel);
        }
    }
    fn clear_diagnostics(&mut self) {
        let old = std::mem::take(&mut self.diagnostics);
        if !old.is_empty() {
            self.retire_lsp(old);
        }
    }
    fn retire_lsp(&mut self, value: impl Send + 'static) {
        self.preparation
            .get_or_insert_with(|| crate::background::KeyedWorker::new(prepare))
            .retire(value);
    }
    fn submit_preparation(&mut self, key: PreparationKey, job: PreparationJob) {
        self.preparation
            .get_or_insert_with(|| crate::background::KeyedWorker::new(prepare))
            .submit(key, job);
    }
    fn prepared(&mut self, app: &mut App, result: Prepared) -> bool {
        if result.epoch != self.preparation_epoch
            || self
                .servers
                .get(&result.server)
                .is_none_or(|s| s.status != "Ready")
        {
            self.retire_lsp(result);
            return false;
        }
        match (result.context, result.content) {
            (
                PreparationContext::Diagnostics {
                    key,
                    revision,
                    document,
                },
                PreparedContent::Diagnostics(diagnostics),
            ) => {
                if let Some((id, instance)) = document {
                    if app
                        .workspace
                        .buffers
                        .get(&id)
                        .is_none_or(|e| e.document_id != instance || e.buffer.revision != revision)
                    {
                        self.retire_lsp(diagnostics);
                        return false;
                    }
                } else if app.workspace.buffers.values().any(|e| {
                    e.buffer
                        .path
                        .as_ref()
                        .is_some_and(|p| lsp::path_key(p) == key)
                }) {
                    self.retire_lsp(diagnostics);
                    return false;
                }
                if let Some(old) = self.diagnostics.insert(key, diagnostics)
                    && let Some(worker) = &self.preparation
                {
                    worker.retire(old);
                }
            }
            (PreparationContext::Response { id, pending: p }, PreparedContent::Rows(rows)) => {
                if self
                    .prepared_requests
                    .get(&(result.server.clone(), p.buffer, p.kind as u8))
                    != Some(&id)
                    || app.workspace.active_buffer() != p.buffer
                    || app.workspace.buffers.get(&p.buffer).is_none_or(|e| {
                        e.document_id != p.document || e.buffer.revision != p.revision
                    })
                {
                    self.retire_lsp(rows);
                    return false;
                }
                self.prepared_requests
                    .remove(&(result.server, p.buffer, p.kind as u8));
                if rows.len() == 1
                    && p.kind != RequestKind::References
                    && p.kind != RequestKind::Actions
                {
                    if let Choice::Location {
                        path,
                        position,
                        encoding,
                    } = rows[0].choice.clone()
                    {
                        self.task(Task::Location {
                            path,
                            position,
                            encoding,
                        });
                    }
                } else {
                    if let Some(old) = self.panel.take()
                        && let Some(worker) = &self.preparation
                    {
                        worker.retire(old);
                    }
                    self.panel = Some(Panel::list(
                        if p.kind == RequestKind::Actions {
                            "CODE ACTIONS - Enter chooses; edits require confirmation"
                        } else {
                            "LOCATIONS - Enter opens"
                        },
                        if p.kind == RequestKind::Actions {
                            PanelKind::Actions
                        } else {
                            PanelKind::Locations
                        },
                        rows,
                    ));
                }
            }
            _ => return false,
        }
        #[cfg(feature = "perf")]
        crate::perf::trace("lsp_ui_updated", String::new());
        true
    }
    pub fn transport_queues(
        &self,
    ) -> Vec<(String, crate::queue::Snapshot, crate::queue::Snapshot)> {
        self.servers
            .iter()
            .map(|(name, server)| {
                (
                    name.clone(),
                    server.client.input_metrics.snapshot(),
                    server.client.event_metrics.snapshot(),
                )
            })
            .collect()
    }
    pub fn shutdown(&mut self) {
        self.shutting_down = true;
        for server in self.servers.values() {
            server.client.shutdown();
        }
        for session in &self.terminals {
            session.close();
        }
    }
    pub fn stopped(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.servers
            .values()
            .all(|s| s.client.stopped.load(Ordering::Acquire))
            && self
                .terminals
                .iter()
                .all(|s| s.ended.load(Ordering::Acquire))
    }
    pub fn next_wait(&self, app: &App) -> Option<Duration> {
        if self.shutting_down {
            return Some(Duration::from_millis(20));
        }
        let now = Instant::now();
        let mut deadlines = Vec::new();
        if let Some(due) = self.signature_due {
            deadlines.push(due);
        }
        if app.workspace.config.lsp.enabled {
            for (id, (revision, at)) in &self.changes {
                let needs_sync = self.servers.values().any(|s| {
                    s.status == "Ready" && s.documents.get(id).is_some_and(|v| v != revision)
                });
                let completion = *id == app.workspace.active_buffer()
                    && self.contextual_insert(app)
                    && self.completion_at != Some((*id, *revision))
                    && self.prompt.is_none()
                    && self
                        .servers
                        .values()
                        .any(|s| s.status == "Ready" && s.documents.contains_key(id));
                if needs_sync {
                    deadlines
                        .push(*at + Duration::from_millis(app.workspace.config.lsp.debounce_ms));
                }
                if completion {
                    deadlines.push(
                        *at + Duration::from_millis(app.workspace.config.lsp.debounce_ms.min(60)),
                    );
                }
            }
        }
        if app.workspace.config.git.enabled && !self.git_inflight && self.git_error.is_none() {
            if let Some(at) = self.git_dirty {
                deadlines.push(at + Duration::from_millis(app.workspace.config.git.debounce_ms));
            }
            if let Some(at) = self.git_requested {
                deadlines.push(at + Duration::from_millis(app.workspace.config.git.refresh_ms));
            }
        }
        deadlines.into_iter().min().map(|at| {
            at.saturating_duration_since(now)
                .max(Duration::from_millis(1))
        })
    }
    pub fn focused(&self) -> bool {
        self.prompt.is_some()
            || self
                .panel
                .as_ref()
                .is_some_and(|p| p.kind != PanelKind::Completion)
            || self.terminal_focus
    }
    fn contextual_insert(&self, app: &App) -> bool {
        app.workspace.editor().mode == Mode::Insert
            && !self.focused()
            && app.picker.is_none()
            && app.dialog.is_none()
            && !app.explorer.focused
            && !app.workspace.editor().help
    }
    fn task(&mut self, task: Task) {
        let tasks = self.tasks.get_or_insert_with(Tasks::default);
        if tasks.sender.send(task).is_ok() {
            self.busy += 1;
        }
    }
    fn error(&mut self, app: &mut App, error: impl ToString) {
        let s = error.to_string();
        self.message = s.clone();
        app.notify(s, true);
    }
    fn text(&mut self, title: &str, text: impl Into<String>) {
        self.panel = Some(Panel::text(title, text));
        self.terminal_focus = false;
    }
    pub fn command(&mut self, app: &mut App, name: &str, arg: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "settings" => self.settings(app),
            "setupfont" => self.font_scan(false, false),
            "fontstatus" => self.font_scan(false, true),
            "removefont" => self.font_scan(true, false),
            "setup" => {
                self.task(Task::Discover);
                self.setup_panel();
            }
            "lspinfo" => self.info(),
            "lsprestart" => {
                if let Some(worker) = &self.completion_worker {
                    worker.cancel();
                }
                self.invalidate_preparation();
                self.servers.clear();
                self.pending.clear();
                self.changes.clear();
                self.message = "Restarting configured servers".into();
            }
            "hover" => self.request(app, RequestKind::Hover, None),
            "definition" => self.request(app, RequestKind::Definition, None),
            "implementation" => self.request(app, RequestKind::Implementation, None),
            "references" => self.request(app, RequestKind::References, None),
            "rename" => {
                if arg.is_empty() {
                    self.rename()
                } else {
                    self.request(app, RequestKind::Rename, Some(json!({"newName":arg})))
                }
            }
            "format" => self.request(app, RequestKind::Format, None),
            "rangeformat" => self.request(app, RequestKind::RangeFormat, None),
            "codeactions" => self.request(app, RequestKind::Actions, None),
            "signature" => self.request(app, RequestKind::Signature, None),
            "complete" => self.request(app, RequestKind::Completion, None),
            "terminal" => self.toggle_terminal(app, arg == "vertical"),
            "terminalnew" => self.new_terminal(app),
            "terminalnext" => {
                if !self.terminals.is_empty() {
                    self.terminal_active = (self.terminal_active + 1) % self.terminals.len();
                    self.terminal_visible = true;
                    self.terminal_focus = true;
                }
            }
            "terminalclose" => {
                if !self.terminals.is_empty() {
                    self.terminals.remove(self.terminal_active);
                    self.terminal_active = self
                        .terminal_active
                        .min(self.terminals.len().saturating_sub(1));
                }
                self.terminal_focus = false;
                self.terminal_visible = !self.terminals.is_empty();
            }
            "gitstatus" => self.git_command(app, "status"),
            "gitdiff" => self.git_command(app, if arg == "staged" { "staged" } else { "diff" }),
            "gitstage" => self.git_command(app, "stage"),
            "gitunstage" => self.git_command(app, "unstage"),
            "gitprevious" => self.git_command(app, "previous"),
            "gitcommit" => {
                self.prompt = Some(Prompt {
                    title: "Commit staged changes: enter message, then confirm".into(),
                    input: arg.into(),
                    action: PromptAction::Commit,
                })
            }
            "lspinstall" | "lspupdate" => {
                self.prompt = Some(Prompt {
                    title: format!("Type install to download/install or update {arg}"),
                    input: String::new(),
                    action: PromptAction::Install(arg.into()),
                })
            }
            "set" => {
                if let Some((key, value)) = arg.split_once(char::is_whitespace) {
                    self.task(Task::Settings {
                        path: crate::config::Config::path(),
                        key: key.into(),
                        value: value.trim().into(),
                    });
                } else {
                    self.error(app, "Use :Set key value, e.g. :Set visual.fps 60")
                }
            }
            "setkey" => {
                let mut p = arg.splitn(3, ' ');
                if let (Some(mode), Some(key), Some(action)) = (p.next(), p.next(), p.next()) {
                    self.task(Task::Settings {
                        path: crate::config::Config::path(),
                        key: format!("keybindings.{mode}.{key}"),
                        value: action.into(),
                    });
                } else {
                    self.error(
                        app,
                        "Use :SetKey normal KEY ACTION; edit config.toml for leader sequences",
                    )
                }
            }
            "snippets" => self.snippets(app),
            _ => return false,
        }
        true
    }
    pub fn action(&mut self, app: &mut App, action: Action) -> bool {
        use Action::*;
        if action == Normal {
            self.signature = None;
            self.signature_due = None;
            self.completion_accept = None;
        }
        if self.prompt.is_some() {
            match action {
                ClearInput => self.prompt.as_mut().unwrap().input.clear(),
                Normal => {
                    if let Some(Prompt {
                        action: PromptAction::ServerApply { server, id, .. },
                        ..
                    }) = self.prompt.take()
                        && let Some(s) = self.servers.get(&server)
                    {
                        s.client.respond(
                            id,
                            json!({"applied":false,"failureReason":"User cancelled"}),
                        );
                    }
                }
                Backspace => {
                    if let Some(p) = &mut self.prompt {
                        crate::app::pop_grapheme(&mut p.input);
                    }
                }
                Submit => self.submit_prompt(app),
                _ => {}
            }
            return true;
        }
        if let Some(panel) = &mut self.panel {
            let completion = panel.kind == PanelKind::Completion;
            if completion && action == Normal {
                self.clear_panel();
                return false;
            }
            match action {
                Normal => {
                    self.clear_panel();
                    return true;
                }
                PageUp | PageDown if panel.kind == PanelKind::Font => {
                    panel.scroll = if action == PageUp {
                        panel.scroll.saturating_sub(8)
                    } else {
                        panel.scroll.saturating_add(8)
                    };
                    return true;
                }
                Up | Down | PageUp | PageDown => {
                    if panel.kind == PanelKind::Font {
                        panel.scroll = 0;
                    }
                    let n = panel.visible().len();
                    let step = if matches!(action, PageUp | PageDown) {
                        10
                    } else {
                        1
                    };
                    panel.selected = if matches!(action, Up | PageUp) {
                        panel.selected.saturating_sub(step)
                    } else {
                        (panel.selected + step).min(n.saturating_sub(1))
                    };
                    if panel.rows.is_empty() {
                        panel.scroll = if matches!(action, Up | PageUp) {
                            panel.scroll.saturating_sub(step)
                        } else {
                            panel.scroll + step
                        };
                    }
                    return true;
                }
                Submit | Tab | Newline if completion || action != Newline => {
                    self.choose(app);
                    return true;
                }
                Backspace if !completion => {
                    crate::app::pop_grapheme(&mut panel.query);
                    panel.selected = 0;
                    return true;
                }
                _ if !completion => return true,
                _ => {}
            }
        }
        if action == Tab && app.workspace.editor().mode == Mode::Insert && self.next_field(app) {
            return true;
        }
        match action {
            Complete => self.request(app, RequestKind::Completion, None),
            Hover => self.request(app, RequestKind::Hover, None),
            Definition => self.request(app, RequestKind::Definition, None),
            Implementation => self.request(app, RequestKind::Implementation, None),
            References => self.request(app, RequestKind::References, None),
            RenameSymbol => self.rename(),
            CodeActions => self.request(app, RequestKind::Actions, None),
            FormatDocument => self.request(
                app,
                if app.workspace.editor().selection().is_some() {
                    RequestKind::RangeFormat
                } else {
                    RequestKind::Format
                },
                None,
            ),
            SignatureHelp => self.request(app, RequestKind::Signature, None),
            LspInfo => self.info(),
            Settings => self.settings(app),
            Setup => {
                self.task(Task::Discover);
                self.setup_panel();
            }
            GitStatus => self.git_command(app, "status"),
            GitDiff => self.git_command(app, "diff"),
            GitStage => self.git_command(app, "stage"),
            GitUnstage => self.git_command(app, "unstage"),
            Terminal => self.toggle_terminal(app, false),
            Snippets => self.snippets(app),
            _ => return false,
        }
        true
    }
    pub fn type_text(&mut self, app: &mut App, text: &str) -> bool {
        if let Some(prompt) = &mut self.prompt {
            prompt.input.push_str(text);
            return true;
        }
        if let Some(panel) = &mut self.panel
            && panel.kind != PanelKind::Completion
        {
            panel.query.push_str(text);
            panel.selected = 0;
            return true;
        }
        if let Some(snippet) = &mut self.snippet
            && snippet.buffer == app.workspace.active_buffer()
            && snippet.selected
            && app.workspace.editor().mode == Mode::Insert
            && let Some(range) = snippet.fields.get(snippet.index).cloned()
        {
            let before = app.workspace.editor().buffer.revision;
            app.workspace
                .external_edits(snippet.buffer, &[(range.clone(), text.into())]);
            let delta = text.chars().count() as isize - (range.end - range.start) as isize;
            for field in snippet.fields.iter_mut().skip(snippet.index + 1) {
                field.start = field.start.saturating_add_signed(delta);
                field.end = field.end.saturating_add_signed(delta);
            }
            snippet.selected = false;
            snippet.revision = before + 1;
            app.workspace.editor_mut().anchor = None;
            return true;
        }
        false
    }
    fn rename(&mut self) {
        self.prompt = Some(Prompt {
            title: "Rename symbol to".into(),
            input: String::new(),
            action: PromptAction::Rename,
        });
    }
    fn submit_prompt(&mut self, app: &mut App) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        match prompt.action {
            PromptAction::Font(operation) => {
                let confirmation = if matches!(operation, font::Operation::Remove) {
                    "remove"
                } else {
                    "apply"
                };
                if prompt.input.trim() == confirmation {
                    self.clear_panel();
                    self.message = "Font setup running in the background...".into();
                    self.task(Task::Font(operation));
                } else {
                    self.error(app, "Font operation cancelled: confirmation did not match");
                }
            }
            PromptAction::Rename => {
                if !prompt.input.trim().is_empty() {
                    self.request(
                        app,
                        RequestKind::Rename,
                        Some(json!({"newName":prompt.input.trim()})),
                    )
                }
            }
            PromptAction::Setting(key) => self.task(Task::Settings {
                path: crate::config::Config::path(),
                key,
                value: prompt.input,
            }),
            PromptAction::Install(id) => {
                if prompt.input.trim() == "install" {
                    self.message = format!("Installing/updating {id}; checking prerequisites...");
                    self.task(Task::Install(id));
                } else {
                    self.error(app, "Installation cancelled: confirmation must be install")
                }
            }
            PromptAction::Commit => {
                if let Some(message) = prompt.input.strip_prefix("confirm ") {
                    self.git_job(git::Job::Commit {
                        root: app.workspace.root.clone(),
                        message: message.into(),
                    });
                } else {
                    self.prompt = Some(Prompt {
                        title: "Type confirm followed by this message to commit staged changes"
                            .into(),
                        input: format!("confirm {}", prompt.input),
                        action: PromptAction::Commit,
                    });
                }
            }
            PromptAction::Apply(plan) => {
                if prompt.input.trim() == "apply"
                    && let Err(e) = apply_plan(app, plan)
                {
                    self.error(app, e)
                }
            }
            PromptAction::ApplyCommand {
                plan,
                server,
                command,
            } => {
                if prompt.input.trim() == "apply" {
                    match apply_plan(app, plan) {
                        Ok(()) => self.execute_command(app, &server, command),
                        Err(e) => self.error(app, e),
                    }
                }
            }
            PromptAction::ServerApply { server, id, plan } => {
                let result = if prompt.input.trim() == "apply" {
                    apply_plan(app, plan)
                } else {
                    Err(anyhow::anyhow!("User cancelled"))
                };
                if let Some(s) = self.servers.get(&server) {
                    s.client.respond(id,json!({"applied":result.is_ok(),"failureReason":result.err().map(|e|e.to_string())}));
                }
            }
        }
    }
    fn choose(&mut self, app: &mut App) {
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        let Some(row) = panel.visible().get(panel.selected).map(|r| (*r).clone()) else {
            return;
        };
        let kind = panel.kind;
        match row.choice {
            Choice::Font(operation) => {
                let title = match &operation {
                    font::Operation::Remove => "Type remove to delete only the verified Filavim-managed font; terminal settings stay unchanged".into(),
                    font::Operation::Install => "Type apply to install the bundled font for this user; terminal font stays unchanged".into(),
                    font::Operation::Configure(target) => format!("Type apply to back up and set font.face: {} / {}", target.path.display(), target.label),
                    font::Operation::Both(target) => format!("Type apply to install + back up/set font.face: {} / {}", target.path.display(), target.label),
                };
                self.prompt = Some(Prompt {
                    title,
                    input: String::new(),
                    action: PromptAction::Font(operation),
                });
            }
            Choice::FontSetup => self.font_scan(false, false),
            Choice::Cancel => self.clear_panel(),
            Choice::Completion(value) => self.accept_completion(app, value),
            Choice::CodeAction { server, value } => self.apply_action(app, server, value),
            Choice::Location {
                path,
                position,
                encoding,
            } => {
                self.clear_panel();
                self.task(Task::Location {
                    path,
                    position,
                    encoding,
                });
            }
            Choice::Setting(key, value) => {
                self.prompt = Some(Prompt {
                    title: format!("Set {key} (TOML value; Enter validates and saves)"),
                    input: value,
                    action: PromptAction::Setting(key),
                })
            }
            Choice::Install(id) => {
                self.prompt = Some(Prompt {
                    title: format!("Type install to download/install or update {id}"),
                    input: String::new(),
                    action: PromptAction::Install(id),
                })
            }
            Choice::GitFile(path) => {
                self.clear_panel();
                self.git_job(git::Job::Diff {
                    root: app.workspace.root.clone(),
                    path,
                    staged: false,
                });
            }
            Choice::Hunk(index) => {
                if let Some(hunk) = self.git.hunks.get(index) {
                    self.prompt = None;
                    let mut p = Panel::text(
                        "GIT HUNK - :GitStage / :GitUnstage stage whole file; Enter stages this hunk",
                        hunk.patch.clone(),
                    );
                    p.kind = PanelKind::Diff;
                    p.rows = vec![Row {
                        label: if self.git.staged {
                            "Unstage this hunk"
                        } else {
                            "Stage this hunk"
                        }
                        .into(),
                        detail: hunk.patch.clone(),
                        choice: Choice::Hunk(index),
                    }];
                    if kind == PanelKind::Diff
                        && self.panel.as_ref().is_some_and(|p| p.rows.len() == 1)
                    {
                        let path = self.git.path.clone().unwrap();
                        self.git_job(git::Job::Hunk {
                            root: self.git.root.clone(),
                            path,
                            patch: hunk.patch.clone(),
                            undo: self.git.staged,
                        });
                        self.clear_panel();
                    } else {
                        self.panel = Some(p);
                    }
                }
            }
            Choice::Snippet(source) => {
                self.clear_panel();
                self.insert_snippet(app, &source, None);
            }
            Choice::None => {}
        }
    }
    fn settings(&mut self, app: &App) {
        let c = &app.workspace.config;
        let entries = [
            ("theme_name", format!("\"{}\"", c.theme_name)),
            ("preset", format!("\"{:?}\"", c.preset).to_lowercase()),
            ("visual.cursor_animation", c.effects().cursor.to_string()),
            (
                "visual.cursor_effect",
                format!("\"{:?}\"", c.visual.cursor_effect).to_lowercase(),
            ),
            (
                "visual.pixiedust.density",
                c.visual.pixiedust.density.to_string(),
            ),
            (
                "visual.pixiedust.lifetime_ms",
                c.visual.pixiedust.lifetime_ms.to_string(),
            ),
            ("visual.pixiedust.fps", c.visual.pixiedust.fps.to_string()),
            ("visual.fps", c.effects().fps.to_string()),
            ("visual.trail_intensity", c.effects().trail.to_string()),
            (
                "visual.smear.stiffness",
                c.visual.smear.stiffness.to_string(),
            ),
            (
                "visual.smear.trailing_stiffness",
                c.visual.smear.trailing_stiffness.to_string(),
            ),
            ("visual.smear.damping", c.visual.smear.damping.to_string()),
            (
                "visual.smear.gradient_exponent",
                c.visual.smear.gradient_exponent.to_string(),
            ),
            (
                "visual.smear.max_length",
                c.visual.smear.max_length.to_string(),
            ),
            (
                "visual.smear.quality",
                format!("\"{:?}\"", c.visual.smear.quality).to_lowercase(),
            ),
            ("visual.smooth_scroll", c.effects().scroll.to_string()),
            (
                "theme.base",
                format!(
                    "\"{}\"",
                    c.theme.get("base").map_or("#232136", String::as_str)
                ),
            ),
            (
                "theme.iris",
                format!(
                    "\"{}\"",
                    c.theme.get("iris").map_or("#c4a7e7", String::as_str)
                ),
            ),
            ("line_numbers", c.line_numbers.to_string()),
            ("borders", c.borders.to_string()),
            ("status_bar", c.status_bar.to_string()),
            ("nerd_fonts", c.nerd_fonts.to_string()),
            ("unicode", c.unicode.to_string()),
            ("tab_width", c.tab_width.to_string()),
            (
                "navigation.explorer_width",
                c.navigation.explorer_width.to_string(),
            ),
            (
                "navigation.show_hidden",
                c.navigation.show_hidden.to_string(),
            ),
            ("lsp.enabled", c.lsp.enabled.to_string()),
            ("lsp.auto_start", c.lsp.auto_start.to_string()),
            ("git.enabled", c.git.enabled.to_string()),
            ("terminal.enabled", c.terminal.enabled.to_string()),
            ("terminal.shell", format!("{:?}", c.terminal.shell)),
            ("keybindings.normal.w", "\"up\"".into()),
            ("keybindings.normal.a", "\"left\"".into()),
            ("keybindings.normal.s", "\"down\"".into()),
            ("keybindings.normal.d", "\"right\"".into()),
        ];
        let mut rows = entries.into_iter().map(|(key,value)|Row{label:format!("{key} = {value}"),detail:"Enter edits; saved settings apply without restarting. :Set supports any configuration key.".into(),choice:Choice::Setting(key.into(),value)}).collect::<Vec<_>>();
        for server in &c.lsp.servers {
            for (property, value) in [
                ("command", server.command.clone()),
                ("enabled", server.enabled.to_string()),
                ("languages", server.languages.join(",")),
            ] {
                let key = format!("server.{}.{property}", server.name);
                rows.push(Row{label:format!("{key} = {value}"),detail:"Enter edits server configuration; changes apply without restarting Filavim.".into(),choice:Choice::Setting(key,value)});
            }
        }
        self.panel = Some(Panel::list(
            "SETTINGS - type to filter; Enter edit; Esc close",
            PanelKind::Settings,
            rows,
        ));
    }
    fn setup_panel(&mut self) {
        let mut rows = setup::PACKAGES
            .iter()
            .map(|p| Row {
                label: format!(
                    "{} - {}",
                    p.id,
                    self.installations
                        .iter()
                        .find(|(id, _)| id == p.id)
                        .map_or("Checking...", |(_, s)| s.as_str())
                ),
                detail: p.description.into(),
                choice: Choice::Install(p.id.into()),
            })
            .collect::<Vec<_>>();
        rows.push(Row{label:"font-fira-code - install OFL-licensed Fira Code 6.2".into(),detail:"Choose Fira Code in your terminal after installation. MonoLisa: supply your own licensed copy; never downloaded by Filavim. Nerd Font icons are optional; Unicode/ASCII fallbacks are default.".into(),choice:Choice::Install("font-fira-code".into())});
        rows.push(Row { label:"Bundled font setup (:SetupFont)".into(), detail:"Inspect the private development font, per-user installation, and terminal configuration. Opening setup makes no system changes.".into(), choice:Choice::FontSetup });
        self.panel = Some(Panel::list(
            "SETUP - Enter requests confirmation; no automatic installations",
            PanelKind::Setup,
            rows,
        ));
    }
    fn font_scan(&mut self, remove: bool, readonly: bool) {
        self.font_generation = self.font_generation.wrapping_add(1);
        let mut panel = Panel::text(
            "FONT SETUP - inspecting in background; Esc cancels",
            "Reading font and terminal status. No system changes are made by inspection.",
        );
        panel.kind = PanelKind::Font;
        self.panel = Some(panel);
        self.task(Task::FontStatus {
            generation: self.font_generation,
            remove,
            readonly,
        });
    }
    fn font_panel(&mut self, report: font::Report, remove: bool, readonly: bool) {
        let mut rows = vec![Row {
            label: "Review font and terminal status".into(),
            detail: report.summary.clone(),
            choice: Choice::None,
        }];
        if !readonly {
            if remove {
                rows.push(Row { label:if report.managed {"Remove managed font (confirmation required)"} else {"No verified managed font to remove"}.into(), detail:format!("{}\n\nOnly the exact managed file and matching registration can be removed. Terminal settings and backups stay unchanged.",report.summary), choice:if report.managed {Choice::Font(font::Operation::Remove)} else {Choice::None} });
            } else {
                rows.push(Row {
                    label: if report.available {
                        "Install font for current user"
                    } else {
                        "Install unavailable: development bundle absent"
                    }
                    .into(),
                    detail: report.summary.clone(),
                    choice: if report.available {
                        Choice::Font(font::Operation::Install)
                    } else {
                        Choice::None
                    },
                });
                if report.targets.is_empty() {
                    rows.push(Row {
                        label: "Configure terminal manually (instructions)".into(),
                        detail: report.summary.clone(),
                        choice: Choice::None,
                    });
                }
                for target in report.targets {
                    let detail = format!(
                        "Configuration file: {}\nScope: {}\nConfigured font: {}\n\n{}",
                        target.path.display(),
                        target.label,
                        target.current.as_deref().unwrap_or("unknown/default"),
                        report.summary
                    );
                    rows.push(Row {
                        label: format!(
                            "Configure{}: {} [{}]",
                            if report.installed {
                                ""
                            } else {
                                " (install first)"
                            },
                            target.label,
                            target.path.display()
                        ),
                        detail: detail.clone(),
                        choice: if report.installed {
                            Choice::Font(font::Operation::Configure(target.clone()))
                        } else {
                            Choice::None
                        },
                    });
                    if report.available {
                        rows.push(Row {
                            label: format!(
                                "Install + configure: {} [{}]",
                                target.label,
                                target.path.display()
                            ),
                            detail,
                            choice: Choice::Font(font::Operation::Both(target)),
                        });
                    }
                }
            }
        }
        rows.push(Row {
            label: "Cancel / close".into(),
            detail: "Close this panel. No changes are made.".into(),
            choice: Choice::Cancel,
        });
        self.panel = Some(Panel::list(
            if readonly {
                "FONT STATUS"
            } else if remove {
                "REMOVE FONT"
            } else {
                "SETUP FONT"
            },
            PanelKind::Font,
            rows,
        ));
    }
    fn info(&mut self) {
        let mut text = String::new();
        for (name, s) in &self.servers {
            text.push_str(&format!(
                "{name}: {}\nencoding: {:?}\ncapabilities: {}\n\n",
                s.status, s.encoding, s.capabilities
            ));
        }
        if text.is_empty() {
            text="No server started yet. Open a supported file; :Setup discovers installations. Configure [lsp] and [[lsp.servers]] in config.toml.\nRust, Python, TS/JS, C/C++, Go and JSON are supported by the default registry.".into()
        }
        self.text("LSP INFORMATION", text);
    }
    fn snippets(&mut self, app: &App) {
        let language = app
            .workspace
            .editor()
            .buffer
            .path
            .as_deref()
            .and_then(lsp::language)
            .unwrap_or("");
        let mut rows = crate::snippet::defaults(language)
            .into_iter()
            .map(|(name, text)| Row {
                label: name.into(),
                detail: text.into(),
                choice: Choice::Snippet(text.into()),
            })
            .collect::<Vec<_>>();
        if let Some(custom) = app.workspace.config.snippets.get(language) {
            rows.extend(custom.iter().map(|(name, text)| Row {
                label: name.clone(),
                detail: text.clone(),
                choice: Choice::Snippet(text.clone()),
            }));
        }
        self.panel = Some(Panel::list(
            "SNIPPETS - Enter expands; Tab advances fields",
            PanelKind::Snippets,
            rows,
        ));
    }
    fn new_terminal(&mut self, app: &mut App) {
        if !app.workspace.config.terminal.enabled {
            self.error(app, "Terminal disabled in settings");
            return;
        }
        self.terminals.push(terminal::Session::start(
            app.workspace.config.terminal.clone(),
            app.workspace.root.clone(),
            12,
            80,
        ));
        self.terminal_active = self.terminals.len() - 1;
        self.terminal_visible = true;
        self.terminal_focus = true;
        self.clear_panel();
    }
    fn toggle_terminal(&mut self, app: &mut App, vertical: bool) {
        self.terminal_vertical = vertical;
        if self.terminals.is_empty() {
            self.new_terminal(app)
        } else if self.terminal_visible && self.terminal_focus {
            self.terminal_visible = false;
            self.terminal_focus = false;
        } else {
            self.terminal_visible = true;
            self.terminal_focus = true;
            self.clear_panel();
        }
    }
    fn git_job(&mut self, job: git::Job) {
        if self
            .git_worker
            .get_or_insert_with(git::Service::default)
            .sender
            .send(job)
            .is_ok()
        {
            self.git_inflight = true;
        }
    }
    fn git_command(&mut self, app: &mut App, kind: &str) {
        if !app.workspace.config.git.enabled {
            self.error(app, "Git disabled");
            return;
        }
        let root = app.workspace.root.clone();
        self.git_error = None;
        let path = app.workspace.editor().buffer.path.clone();
        match kind {
            "status" => {
                self.git_job(git::Job::Refresh {
                    root,
                    path,
                    text: None,
                    revision: 0,
                });
                self.panel = Some(Panel::list(
                    "GIT STATUS - Enter views file diff; :GitStage / :GitUnstage",
                    PanelKind::Git,
                    Vec::new(),
                ));
            }
            _ => {
                let Some(path) = path else {
                    self.error(app, "Save the buffer to a file first");
                    return;
                };
                if matches!(kind, "stage" | "unstage") && app.workspace.editor().buffer.dirty() {
                    self.error(
                        app,
                        "Save unsaved edits before staging/unstaging their on-disk file",
                    );
                    return;
                }
                match kind {
                    "diff" | "staged" => self.git_job(git::Job::Diff {
                        root,
                        path,
                        staged: kind == "staged",
                    }),
                    "stage" | "unstage" => self.git_job(git::Job::Stage {
                        root,
                        path,
                        undo: kind == "unstage",
                    }),
                    "previous" => self.git_job(git::Job::Previous { root, path }),
                    _ => {}
                }
            }
        }
    }
    fn server_for(&mut self, app: &App, buffer: BufferId, force: bool) -> Option<String> {
        let path = app.workspace.buffers.get(&buffer)?.buffer.path.as_ref()?;
        let language = lsp::language(path)?;
        let config = app
            .workspace
            .config
            .lsp
            .servers
            .iter()
            .find(|s| s.enabled && s.languages.iter().any(|l| l == language))?;
        let name = config.name.clone();
        if !self.servers.contains_key(&name) {
            if !force && !app.workspace.config.lsp.auto_start {
                return None;
            }
            self.servers.insert(
                name.clone(),
                Server {
                    client: lsp::Client::start(
                        config.clone(),
                        app.workspace.root.clone(),
                        Duration::from_millis(app.workspace.config.lsp.timeout_ms),
                    ),
                    capabilities: Value::Null,
                    encoding: Encoding::Utf16,
                    status: "Starting".into(),
                    documents: BTreeMap::new(),
                    uris: BTreeMap::new(),
                    paths: BTreeMap::new(),
                    instances: BTreeMap::new(),
                },
            );
        }
        Some(name)
    }
    fn sync(&mut self, app: &App, buffer: BufferId, server: &str) {
        let editor = &app.workspace.buffers[&buffer];
        let Some(path) = &editor.buffer.path else {
            return;
        };
        let cached = &self.servers[server];
        if cached.documents.get(&buffer) == Some(&editor.buffer.revision)
            && cached.instances.get(&buffer) == Some(&editor.document_id)
            && cached.paths.get(&buffer) == Some(path)
        {
            return;
        }
        let Ok(uri) = lsp::uri(path) else { return };
        let slot = self.servers.get_mut(server).unwrap();
        let revision = editor.buffer.revision;
        if slot.uris.get(&buffer).is_some_and(|old| old != &uri)
            || slot
                .instances
                .get(&buffer)
                .is_some_and(|old| *old != editor.document_id)
        {
            if let Some(old) = slot.uris.remove(&buffer) {
                slot.client
                    .notify("textDocument/didClose", json!({"textDocument":{"uri":old}}));
            }
            slot.documents.remove(&buffer);
        }
        if slot.documents.get(&buffer) != Some(&revision) {
            let open = !slot.documents.contains_key(&buffer);
            let changes = slot.documents.get(&buffer).and_then(|revision| {
                editor
                    .buffer
                    .changes_since(*revision)
                    .map(|edits| (*revision, edits))
            });
            #[cfg(feature = "perf")]
            crate::perf::trace("lsp_sync_scheduled", format!("{uri}@{revision}"));
            slot.client.document_changes(
                uri.clone(),
                lsp::language(path).unwrap_or("").into(),
                revision,
                editor.buffer.text.clone(),
                open,
                changes,
            );
            slot.documents.insert(buffer, revision);
            slot.uris.insert(buffer, uri.clone());
            slot.paths.insert(buffer, path.clone());
            slot.instances.insert(buffer, editor.document_id);
        }
    }
    pub fn request(&mut self, app: &mut App, kind: RequestKind, extra: Option<Value>) {
        if !app.workspace.config.lsp.enabled {
            self.error(app, "LSP disabled in settings");
            return;
        }
        let buffer = app.workspace.active_buffer();
        let Some(server) = self.server_for(app, buffer, true) else {
            self.error(app, "No language server configured for this file");
            return;
        };
        self.sync(app, buffer, &server);
        let slot = &self.servers[&server];
        if slot.status != "Ready" {
            self.message = format!("{}: {} - :LspInfo / :Setup", server, slot.status);
            app.notify(self.message.clone(), false);
            return;
        }
        let (method, capability) = match kind {
            RequestKind::Completion => ("textDocument/completion", "completionProvider"),
            RequestKind::Resolve => ("completionItem/resolve", "completionProvider"),
            RequestKind::ResolvePreview => ("completionItem/resolve", "completionProvider"),
            RequestKind::Hover => ("textDocument/hover", "hoverProvider"),
            RequestKind::Definition => ("textDocument/definition", "definitionProvider"),
            RequestKind::Implementation => {
                ("textDocument/implementation", "implementationProvider")
            }
            RequestKind::References => ("textDocument/references", "referencesProvider"),
            RequestKind::Rename => ("textDocument/rename", "renameProvider"),
            RequestKind::Actions => ("textDocument/codeAction", "codeActionProvider"),
            RequestKind::ResolveAction => ("codeAction/resolve", "codeActionProvider"),
            RequestKind::Format => ("textDocument/formatting", "documentFormattingProvider"),
            RequestKind::RangeFormat => (
                "textDocument/rangeFormatting",
                "documentRangeFormattingProvider",
            ),
            RequestKind::Signature => ("textDocument/signatureHelp", "signatureHelpProvider"),
            RequestKind::Execute => ("workspace/executeCommand", "executeCommandProvider"),
        };
        if slot.capabilities[capability].is_null() || slot.capabilities[capability] == false {
            self.message = format!("Server does not support {method}");
            if kind != RequestKind::Completion {
                app.notify(self.message.clone(), false);
            }
            return;
        }
        let e = app.workspace.editor();
        let encoding = slot.encoding;
        let uri = lsp::uri(e.buffer.path.as_deref().unwrap()).unwrap();
        let pos = lsp::position(&e.buffer.text, e.buffer.cursor, encoding);
        let mut params = json!({"textDocument":{"uri":uri},"position":pos});
        match kind {
            RequestKind::References => params["context"] = json!({"includeDeclaration":true}),
            RequestKind::Actions => {
                let range = e.selection().unwrap_or(e.buffer.cursor..e.buffer.cursor);
                params["range"] = json!(lsp::selection_range(&e.buffer.text, range, encoding));
                params["context"] = json!({"diagnostics":self.diagnostics.get(&lsp::path_key(e.buffer.path.as_deref().unwrap())).map(|d|d.iter().map(|d|d.raw.clone()).collect::<Vec<_>>()).unwrap_or_default()});
            }
            RequestKind::Format | RequestKind::RangeFormat => {
                params = json!({"textDocument":{"uri":uri},"options":{"tabSize":e.config.tab_width,"insertSpaces":e.config.expand_tabs}});
                if kind == RequestKind::RangeFormat {
                    params["range"] = json!(lsp::selection_range(
                        &e.buffer.text,
                        e.selection().unwrap_or(0..e.buffer.text.len_chars()),
                        encoding
                    ));
                }
            }
            _ => {}
        }
        if let Some(extra) = extra {
            if matches!(
                kind,
                RequestKind::Resolve
                    | RequestKind::ResolvePreview
                    | RequestKind::ResolveAction
                    | RequestKind::Execute
            ) {
                params = extra
            } else if let Some(fields) = extra.as_object() {
                for (k, v) in fields {
                    params[k] = v.clone();
                }
            }
        }
        let stale = self
            .pending
            .iter()
            .filter(|(_, p)| p.buffer == buffer && p.kind == kind)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(p) = self.pending.remove(&id) {
                self.servers[&p.server].client.cancel(id);
            }
        }
        self.serial += 1;
        if matches!(
            kind,
            RequestKind::Definition
                | RequestKind::Implementation
                | RequestKind::References
                | RequestKind::Actions
        ) {
            self.prepared_requests
                .insert((server.clone(), buffer, kind as u8), self.serial);
        }
        self.servers[&server]
            .client
            .request(self.serial, method, params);
        self.pending.insert(
            self.serial,
            Pending {
                server,
                buffer,
                revision: e.buffer.revision,
                cursor: e.buffer.cursor,
                kind,
                document: e.document_id,
                mode: e.mode,
            },
        );
        if kind == RequestKind::Completion {
            self.completion_at = Some((buffer, e.buffer.revision));
        }
    }
    fn prepare_edit(
        &mut self,
        app: &App,
        edit: Value,
        encoding: Encoding,
        source: Option<(String, Value)>,
    ) {
        self.prepare_edit_followup(app, edit, encoding, source, None);
    }
    fn prepare_edit_followup(
        &mut self,
        app: &App,
        edit: Value,
        encoding: Encoding,
        source: Option<(String, Value)>,
        followup: Option<(String, Value)>,
    ) {
        let snapshots = app
            .workspace
            .buffers
            .values()
            .filter_map(|e| {
                Some((
                    lsp::path_key(e.buffer.path.as_ref()?),
                    lsp::Snapshot {
                        text: e.buffer.text.clone(),
                        revision: e.buffer.revision,
                    },
                ))
            })
            .collect();
        self.task(Task::Plan {
            edit,
            snapshots,
            encoding,
            source,
            followup,
        });
    }
    fn apply_action(&mut self, app: &mut App, server: String, value: Value) {
        self.clear_panel();
        if value.get("disabled").is_some() {
            self.error(app, "Code action is disabled by the server");
            return;
        }
        if let Some(edit) = value.get("edit") {
            let encoding = self.servers[&server].encoding;
            let followup = value.get("command").map(|command| {
                (
                    server.clone(),
                    if command.is_object() {
                        command.clone()
                    } else {
                        value.clone()
                    },
                )
            });
            self.prepare_edit_followup(app, edit.clone(), encoding, None, followup);
            return;
        } else if value.get("data").is_some()
            && self.servers[&server].capabilities["codeActionProvider"]["resolveProvider"] == true
        {
            self.request(app, RequestKind::ResolveAction, Some(value));
            return;
        }
        if let Some(command) = value.get("command") {
            if command.is_object() {
                self.execute_command(app, &server, command.clone())
            } else if command.is_string() {
                self.execute_command(app, &server, value);
            }
        }
    }
    fn execute_command(&mut self, app: &mut App, server: &str, command: Value) {
        let Some(slot) = self.servers.get(server) else {
            return;
        };
        self.serial += 1;
        slot.client
            .request(self.serial, "workspace/executeCommand", command);
        let e = app.workspace.editor();
        self.pending.insert(
            self.serial,
            Pending {
                server: server.into(),
                buffer: app.workspace.active_buffer(),
                revision: e.buffer.revision,
                cursor: e.buffer.cursor,
                document: e.document_id,
                kind: RequestKind::Execute,
                mode: e.mode,
            },
        );
    }
    fn accept_completion(&mut self, app: &mut App, mut value: Value) {
        let Some((buffer, revision, cursor, server, encoding)) = self.completion_context.clone()
        else {
            return;
        };
        if app.workspace.active_buffer() != buffer
            || app.workspace.editor().mode != Mode::Insert
            || self.completion_document != Some(app.workspace.editor().document_id)
        {
            self.clear_panel();
            return;
        }
        let mut cursor = cursor;
        if app.workspace.editor().buffer.revision != revision
            || app.workspace.editor().buffer.cursor != cursor
        {
            let rebased = self.completion_text.as_ref().and_then(|before| {
                rebase_completion(
                    &value,
                    before,
                    &app.workspace.editor().buffer,
                    revision,
                    cursor,
                    encoding,
                )
            });
            if let Some(rebased) = rebased {
                value = rebased;
                cursor = app.workspace.editor().buffer.cursor;
                self.completion_context = Some((
                    buffer,
                    app.workspace.editor().buffer.revision,
                    cursor,
                    server.clone(),
                    encoding,
                ));
                self.completion_text = Some(app.workspace.editor().buffer.text.clone());
            } else {
                self.completion_accept = Some((
                    buffer,
                    app.workspace.editor().buffer.revision,
                    app.workspace.editor().buffer.cursor,
                ));
                self.request(app, RequestKind::Completion, None);
                return;
            }
        }
        if value["documentation"].is_null()
            && value["detail"].is_null()
            && value.get("data").is_some()
            && self.servers[&server].capabilities["completionProvider"]["resolveProvider"] == true
        {
            self.request(app, RequestKind::Resolve, Some(value));
            return;
        }
        let e = app.workspace.editor();
        let mut range = word_range(&e.buffer.text, cursor);
        let mut text = value["insertText"]
            .as_str()
            .or(value["label"].as_str())
            .unwrap_or("")
            .to_owned();
        if let Some(edit) = value.get("textEdit") {
            let r = edit.get("range").or_else(|| edit.get("replace"));
            if let Some(r) = r
                && let Ok(r) = serde_json::from_value::<lsp_types::Range>(r.clone())
            {
                match (
                    lsp::offset(&e.buffer.text, r.start, encoding),
                    lsp::offset(&e.buffer.text, r.end, encoding),
                ) {
                    (Ok(a), Ok(b)) if a <= b => range = a..b,
                    _ => {
                        self.error(app, "Invalid completion edit range");
                        return;
                    }
                }
            }
            text = edit["newText"].as_str().unwrap_or(&text).to_owned();
        }
        let expansion = if value["insertTextFormat"] == 2 {
            match crate::snippet::expand(&text) {
                Ok(ex) => Some(crate::snippet::normalize(ex, e.buffer.newline)),
                Err(error) => {
                    self.error(app, error);
                    return;
                }
            }
        } else {
            None
        };
        if let Some(ex) = &expansion {
            text = ex.text.clone();
        }
        text = e.buffer.normalize_paste(&text);
        let mut edits = vec![(range.clone(), text.clone())];
        if let Some(extra) = value.get("additionalTextEdits") {
            match serde_json::from_value::<Vec<lsp_types::TextEdit>>(extra.clone())
                .ok()
                .and_then(|edits| lsp::ranges(&e.buffer.text, &edits, encoding).ok())
            {
                Some(extra) => edits.extend(
                    extra
                        .into_iter()
                        .map(|(range, text)| (range, e.buffer.normalize_paste(&text))),
                ),
                None => {
                    self.error(app, "Invalid additional completion edits");
                    return;
                }
            }
        }
        edits.sort_by_key(|(r, _)| (r.start, r.end));
        if edits.windows(2).any(|p| p[0].0.end > p[1].0.start) {
            self.error(app, "Overlapping completion edits");
            return;
        }
        let shift = edits
            .iter()
            .filter(|(r, _)| r.start < range.start)
            .map(|(r, t)| t.chars().count() as isize - (r.end - r.start) as isize)
            .sum::<isize>();
        let base = range.start.saturating_add_signed(shift);
        edits.reverse();
        app.workspace.external_edits(buffer, &edits);
        app.workspace.editor_mut().buffer.cursor = base + text.chars().count();
        app.workspace.stash();
        self.clear_panel();
        if let Some(ex) = expansion {
            self.start_fields(app, base, ex.fields);
        }
        if let Some(command) = value.get("command") {
            self.request(app, RequestKind::Execute, Some(command.clone()));
        }
    }
    fn insert_snippet(&mut self, app: &mut App, source: &str, range: Option<Range<usize>>) {
        match crate::snippet::expand(source) {
            Ok(ex) => {
                let ex = crate::snippet::normalize(ex, app.workspace.editor().buffer.newline);
                let buffer = app.workspace.active_buffer();
                let cursor = app.workspace.editor().buffer.cursor;
                let range = range.unwrap_or(cursor..cursor);
                let base = range.start;
                app.workspace.external_edits(buffer, &[(range, ex.text)]);
                app.workspace.editor_mut().set_mode(Mode::Insert);
                self.start_fields(app, base, ex.fields);
            }
            Err(e) => self.error(app, e),
        }
    }
    fn start_fields(&mut self, app: &mut App, base: usize, fields: Vec<Range<usize>>) {
        self.snippet = Some(Snippet {
            buffer: app.workspace.active_buffer(),
            fields: fields
                .into_iter()
                .map(|r| base + r.start..base + r.end)
                .collect(),
            index: 0,
            selected: true,
            revision: app.workspace.editor().buffer.revision,
        });
        self.select_field(app);
    }
    fn select_field(&mut self, app: &mut App) {
        if let Some(s) = &self.snippet
            && let Some(field) = s.fields.get(s.index)
        {
            let e = app.workspace.editor_mut();
            e.buffer.cursor = field.start;
            if field.end > field.start {
                e.anchor = Some(e.buffer.previous(field.end));
            } else {
                e.anchor = None
            }
            app.workspace.stash();
        }
    }
    fn next_field(&mut self, app: &mut App) -> bool {
        let Some(s) = &mut self.snippet else {
            return false;
        };
        if s.buffer != app.workspace.active_buffer() {
            self.snippet = None;
            return false;
        }
        s.index += 1;
        s.selected = true;
        if s.index >= s.fields.len() {
            self.snippet = None;
            app.workspace.editor_mut().anchor = None;
            return true;
        }
        self.select_field(app);
        true
    }
    pub fn tick(&mut self, app: &mut App) -> bool {
        if self.shutting_down {
            return false;
        }
        let mut changed = false;
        let now = Instant::now();
        if self.root.as_ref() != Some(&app.workspace.root) {
            self.root = Some(app.workspace.root.clone());
            if let Some(worker) = &self.completion_worker {
                worker.cancel();
            }
            self.invalidate_preparation();
            self.servers.clear();
            self.pending.clear();
            self.clear_diagnostics();
            self.changes.clear();
            self.git_stamp = None;
            self.git_requested = None;
            self.git = git::State::default();
            self.git_error = None;
            self.signature = None;
            self.signature_due = None;
            self.signature_seen = None;
        }
        if !app.workspace.config.lsp.enabled
            && (!self.servers.is_empty()
                || !self.pending.is_empty()
                || !self.diagnostics.is_empty())
        {
            if let Some(worker) = &self.completion_worker {
                worker.cancel();
            }
            self.invalidate_preparation();
            self.servers.clear();
            self.pending.clear();
            self.clear_diagnostics();
        }
        self.changes
            .retain(|id, _| app.workspace.buffers.contains_key(id));
        self.prepared_requests
            .retain(|(_, id, _), _| app.workspace.buffers.contains_key(id));
        if self
            .panel
            .as_ref()
            .is_none_or(|p| p.kind != PanelKind::Completion)
        {
            self.completion_text = None;
            self.completion_context = None;
            self.completion_start = None;
            self.completion_document = None;
            self.completion_preview = None;
        }
        let buffer = app.workspace.active_buffer();
        let revision = app.workspace.editor().buffer.revision;
        let cursor = app.workspace.editor().buffer.cursor;
        let document = app.workspace.editor().document_id;
        let editor_mode = app.workspace.editor().mode;
        let stamp = (buffer, revision, cursor, document, editor_mode);
        if self.signature_seen != Some(stamp) {
            self.signature_seen = Some(stamp);
            let call = if editor_mode == Mode::Insert && app.workspace.config.lsp.enabled {
                crate::contextual::call_site(&app.workspace.editor().buffer.text, cursor)
            } else {
                None
            };
            if self.signature.as_ref().is_some_and(|s| {
                s.buffer != buffer
                    || s.document != document
                    || s.call.as_ref().map(|c| c.open) != call.as_ref().map(|c| c.open)
            }) {
                self.signature = None;
                changed = true;
            }
            self.signature_due = if editor_mode == Mode::Insert
                && call.is_some()
                && app.workspace.config.lsp.enabled
                && self.contextual_insert(app)
            {
                Some(now + Duration::from_millis(app.workspace.config.lsp.debounce_ms.min(50)))
            } else {
                if self.signature.take().is_some() {
                    changed = true;
                }
                None
            };
        }
        if !app.workspace.config.lsp.enabled {
            self.signature = None;
            self.signature_due = None;
        }
        for slot in self.servers.values_mut() {
            let closed = slot
                .documents
                .keys()
                .filter(|id| !app.workspace.buffers.contains_key(id))
                .copied()
                .collect::<Vec<_>>();
            for id in closed {
                if let Some(uri) = slot.uris.remove(&id) {
                    slot.client
                        .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
                }
                slot.documents.remove(&id);
                slot.instances.remove(&id);
                slot.paths.remove(&id);
            }
        }
        if app.workspace.config.lsp.enabled {
            let ids = app.workspace.buffers.keys().copied().collect::<Vec<_>>();
            for id in ids {
                if let Some(server) = self.server_for(app, id, false) {
                    let current = app.workspace.buffers[&id].buffer.revision;
                    let state = self.changes.entry(id).or_insert((current, now));
                    if state.0 != current {
                        *state = (current, now);
                    }
                    let changed_at = state.1;
                    let due = now.duration_since(changed_at)
                        >= Duration::from_millis(app.workspace.config.lsp.debounce_ms);
                    if !self.servers[&server].documents.contains_key(&id) || due {
                        self.sync(app, id, &server);
                    }
                    if id == buffer
                        && now.duration_since(changed_at)
                            >= Duration::from_millis(app.workspace.config.lsp.debounce_ms.min(60))
                        && self.contextual_insert(app)
                        && self.completion_at != Some((id, current))
                        && self.prompt.is_none()
                    {
                        self.completion_at = Some((id, current));
                        let prefix = word_range(&app.workspace.editor().buffer.text, cursor);
                        let preceding = cursor
                            .checked_sub(1)
                            .map(|i| app.workspace.editor().buffer.text.char(i).to_string());
                        let trigger = preceding.filter(|c| self.servers[&server].capabilities["completionProvider"]["triggerCharacters"].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(c))));
                        if prefix.start < cursor || trigger.is_some() {
                            let context = if let Some(c) = trigger {
                                json!({"triggerKind":2,"triggerCharacter":c})
                            } else {
                                json!({"triggerKind":1})
                            };
                            self.request(
                                app,
                                RequestKind::Completion,
                                Some(json!({"context":context})),
                            );
                        }
                    }
                }
            }
        }
        if self.signature_due.is_some_and(|due| due <= now) {
            self.signature_due = None;
            if self.contextual_insert(app)
                && let Some(server) = self.server_for(app, buffer, false)
                && self.servers[&server].status == "Ready"
                && !self.servers[&server].capabilities["signatureHelpProvider"].is_null()
                && self.servers[&server].capabilities["signatureHelpProvider"] != false
            {
                let provider = &self.servers[&server].capabilities["signatureHelpProvider"];
                let preceding = cursor
                    .checked_sub(1)
                    .map(|i| app.workspace.editor().buffer.text.char(i).to_string());
                let trigger = preceding.filter(|c| {
                    ["triggerCharacters", "retriggerCharacters"]
                        .iter()
                        .any(|key| {
                            provider[*key]
                                .as_array()
                                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(c)))
                        })
                });
                let mut context = json!({"triggerKind":if trigger.is_some() {2} else if self.signature.is_some() {3} else {1},"isRetrigger":self.signature.is_some()});
                if let Some(trigger) = trigger {
                    context["triggerCharacter"] = json!(trigger);
                }
                if let Some(signature) = &self.signature {
                    context["activeSignatureHelp"] = json!(signature.help);
                }
                self.request(
                    app,
                    RequestKind::Signature,
                    Some(json!({"context":context})),
                );
            }
        }
        let obsolete =
            self.pending
                .iter()
                .filter(|(_, p)| {
                    app.workspace.buffers.get(&p.buffer).is_none_or(|e| {
                        e.buffer.revision != p.revision || e.document_id != p.document
                    }) || matches!(
                        p.kind,
                        RequestKind::Completion
                            | RequestKind::ResolvePreview
                            | RequestKind::Hover
                            | RequestKind::Signature
                    ) && (p.buffer != buffer || p.cursor != cursor || p.mode != editor_mode)
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
        for id in obsolete {
            if let Some(p) = self.pending.remove(&id)
                && let Some(s) = self.servers.get(&p.server)
            {
                s.client.cancel(id);
            }
        }
        if self
            .panel
            .as_ref()
            .is_some_and(|p| p.kind == PanelKind::Completion)
            && self
                .completion_context
                .as_ref()
                .is_none_or(|(id, _, _, _, _)| {
                    *id != buffer
                        || self.completion_document != Some(document)
                        || self.completion_start
                            != Some(word_range(&app.workspace.editor().buffer.text, cursor).start)
                        || app.workspace.editor().mode != Mode::Insert
                })
        {
            self.clear_panel();
            changed = true;
        }
        if let Some(panel) = &mut self.panel
            && panel.kind == PanelKind::Completion
        {
            let prefix = word_range(&app.workspace.editor().buffer.text, cursor);
            let query = app.workspace.editor().buffer.text.slice(prefix).to_string();
            if panel.query != query {
                panel.query = query;
                panel.selected = panel.selected.min(panel.visible().len().saturating_sub(1));
                changed = true;
            }
        }
        let mut budget = crate::background::DrainBudget::new(32);
        let mut names: Vec<_> = self.servers.keys().cloned().collect();
        if !names.is_empty() {
            let turn = self.server_turn % names.len();
            names.rotate_left(turn);
            self.server_turn = (self.server_turn + 1) % names.len();
        }
        for name in names {
            let mut received = 0;
            for _ in 0..8 {
                if !budget.allow() {
                    break;
                }
                let Some(server) = self.servers.get(&name) else {
                    break;
                };
                let Ok(event) = server.client.events.try_recv() else {
                    break;
                };
                received += 1;
                changed |= match &event {
                    lsp::Event::Notification { method, params } => {
                        matches!(method.as_str(), "window/logMessage" | "window/showMessage")
                            && params["message"]
                                .as_str()
                                .is_some_and(|m| m != self.message)
                    }
                    lsp::Event::Reply { id, error, .. } => {
                        error.is_some()
                            || self.pending.get(id).is_some_and(|p| {
                                !matches!(
                                    p.kind,
                                    RequestKind::Completion
                                        | RequestKind::References
                                        | RequestKind::Definition
                                        | RequestKind::Implementation
                                        | RequestKind::Actions
                                )
                            })
                    }
                    _ => true,
                };
                self.event(app, name.clone(), event);
            }
            // More than eight events from one server must not strand the rest.
            if received == 8 {
                crate::background::wake();
            }
        }
        let results = self
            .tasks
            .as_ref()
            .map(|t| t.events.try_iter().take(8).collect::<Vec<_>>())
            .unwrap_or_default();
        if results.len() == 8 {
            crate::background::wake();
        }
        for result in results {
            changed = true;
            self.busy = self.busy.saturating_sub(1);
            match result {
                Err(e) => self.error(app, e),
                Ok(TaskResult::Font(message)) => {
                    self.text("FONT SETUP RESULT", &message);
                    self.message = message;
                }
                Ok(TaskResult::FontStatus {
                    generation,
                    remove,
                    readonly,
                    report,
                }) => {
                    if generation == self.font_generation
                        && self
                            .panel
                            .as_ref()
                            .is_some_and(|p| p.kind == PanelKind::Font)
                    {
                        match report {
                            Ok(report) => self.font_panel(report, remove, readonly),
                            Err(e) => self.error(app, e),
                        }
                    }
                }
                Ok(TaskResult::Message(message)) => {
                    self.text("SETUP RESULT", &message);
                    self.message = message;
                    self.servers.retain(|_, s| s.status == "Ready");
                    self.task(Task::Discover);
                }
                Ok(TaskResult::Discover(rows)) => {
                    self.installations = rows;
                    if self
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.kind == PanelKind::Setup)
                    {
                        self.setup_panel();
                    }
                }
                Ok(TaskResult::Settings(config)) => {
                    let lsp_changed = app.workspace.config.lsp != config.lsp;
                    app.workspace.config = (*config).clone();
                    for e in app.workspace.buffers.values_mut() {
                        e.config = (*config).clone();
                    }
                    self.config_epoch += 1;
                    if lsp_changed {
                        if let Some(worker) = &self.completion_worker {
                            worker.cancel();
                        }
                        self.invalidate_preparation();
                        self.servers.clear();
                        self.pending.clear();
                        self.changes.clear();
                        self.completion_at = None;
                    }
                    self.git_requested = None;
                    self.git_stamp = None;
                    self.git_error = None;
                    self.settings(app);
                    app.refresh();
                    app.notify("Settings validated, saved and applied", false);
                }
                Ok(TaskResult::Plan(result, source, followup)) => {
                    let plan = match result {
                        Ok(plan) => plan,
                        Err(error) => {
                            if let Some((server, id)) = &source
                                && let Some(s) = self.servers.get(server)
                            {
                                s.client.respond(
                                    id.clone(),
                                    json!({"applied":false,"failureReason":error}),
                                );
                            }
                            self.error(app, error);
                            continue;
                        }
                    };
                    if plan.is_empty() {
                        app.notify("No edits returned", false);
                        if let Some((server, id)) = source
                            && let Some(s) = self.servers.get(&server)
                        {
                            s.client.respond(id, json!({"applied":true}));
                        }
                        if let Some((server, command)) = followup {
                            self.execute_command(app, &server, command);
                        }
                    } else {
                        let files = plan
                            .iter()
                            .map(|d| d.path.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.prompt = Some(Prompt {
                            title: format!(
                                "Apply changes to {} file(s)? Type apply. Changes remain unsaved.\n{files}",
                                plan.len()
                            ),
                            input: String::new(),
                            action: match source {
                                Some((server, id)) => {
                                    PromptAction::ServerApply { server, id, plan }
                                }
                                None => match followup {
                                    Some((server, command)) => PromptAction::ApplyCommand {
                                        plan,
                                        server,
                                        command,
                                    },
                                    None => PromptAction::Apply(plan),
                                },
                            },
                        });
                        self.clear_panel();
                    }
                }
                Ok(TaskResult::Location(path, row, column)) => app.open(path, row, column, None),
            }
        }
        if app.workspace.config.git.enabled {
            let stamp = (buffer, revision);
            let last = self.git_stamp.get_or_insert(stamp);
            if *last != stamp {
                *last = stamp;
                self.git_dirty = Some(now);
            }
            let due = self.git_requested.is_none_or(|at| {
                now.duration_since(at) >= Duration::from_millis(app.workspace.config.git.refresh_ms)
            });
            let edit_due = self.git_dirty.is_some_and(|at| {
                now.duration_since(at)
                    >= Duration::from_millis(app.workspace.config.git.debounce_ms)
            });
            if !self.git_inflight && self.git_error.is_none() && (due || edit_due) {
                self.git_requested = Some(now);
                self.git_dirty = None;
                self.git_job(git::Job::Refresh {
                    root: app.workspace.root.clone(),
                    path: app.workspace.editor().buffer.path.clone(),
                    text: Some(app.workspace.editor().buffer.text.clone()),
                    revision,
                });
            }
        }
        let events = self
            .git_worker
            .as_ref()
            .map(|s| s.events.try_iter().take(4).collect::<Vec<_>>())
            .unwrap_or_default();
        if events.len() == 4 {
            crate::background::wake();
        }
        for event in events {
            self.git_inflight = false;
            match event {
                Err(e) => {
                    if self.git_error.as_ref() != Some(&e) {
                        self.git_error = Some(e.clone());
                        self.message = e;
                        changed = true;
                    }
                }
                Ok(git::Event::State {
                    state,
                    revision: rev,
                }) => {
                    self.git_error = None;
                    if state.path.as_deref() == app.workspace.editor().buffer.path.as_deref()
                        && (rev == revision || rev == 0)
                    {
                        changed |= self.git != state;
                        if self.git.lines_unavailable != state.lines_unavailable
                            && let Some(message) = &state.lines_unavailable
                        {
                            app.notify(message.clone(), true);
                        }
                        self.git = state;
                    }
                    if self
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.kind == PanelKind::Git)
                    {
                        self.panel = Some(Panel::list(
                            "GIT STATUS - Enter file diff; Esc closes",
                            PanelKind::Git,
                            self.git
                                .files
                                .iter()
                                .map(|(p, s)| Row {
                                    label: format!(
                                        "{s} {}",
                                        p.strip_prefix(&self.git.root).unwrap_or(p).display()
                                    ),
                                    detail: "Use :GitStage / :GitUnstage on the active file".into(),
                                    choice: Choice::GitFile(p.clone()),
                                })
                                .collect(),
                        ));
                    }
                }
                Ok(git::Event::Diff(state)) => {
                    let rows = state
                        .hunks
                        .iter()
                        .enumerate()
                        .map(|(i, h)| Row {
                            label: h.title.clone(),
                            detail: h.patch.clone(),
                            choice: Choice::Hunk(i),
                        })
                        .collect();
                    self.panel = Some(Panel::list(
                        "GIT DIFF - Enter previews, then Enter stages/unstages hunk",
                        PanelKind::Diff,
                        rows,
                    ));
                    if self.panel.as_ref().is_some_and(|p| p.rows.is_empty()) {
                        self.text("GIT DIFF","No text hunks. Untracked/binary files can be staged as a whole with :GitStage.");
                    }
                    self.git = state;
                    changed = true;
                }
                Ok(git::Event::Previous { path, text }) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.text(&format!("HEAD VERSION: {name}"), text);
                    changed = true;
                }
                Ok(git::Event::Message(message)) => {
                    app.notify(message, false);
                    self.git_requested = None;
                    changed = true;
                }
            }
        }
        for (index, session) in self.terminals.iter_mut().enumerate() {
            session.set_visible(self.terminal_visible && index == self.terminal_active);
            let updated = session.poll();
            changed |= updated && self.terminal_visible && index == self.terminal_active;
        }
        if let Some(s) = &mut self.snippet {
            if s.buffer != buffer || app.workspace.editor().mode != Mode::Insert {
                self.snippet = None;
            } else if s.revision != revision
                && let Some((start, old_end, new_end)) = app.workspace.editor().buffer.last_change
            {
                let delta = new_end as isize - old_end as isize;
                for field in s.fields.iter_mut().skip(s.index + 1) {
                    if field.start >= old_end {
                        field.start = field.start.saturating_add_signed(delta);
                        field.end = field.end.saturating_add_signed(delta);
                    }
                }
                if !s.selected && s.fields[s.index].start > start {
                    self.snippet = None;
                } else {
                    s.revision = revision;
                }
            }
        }
        if let Some(ready) = self.completion_worker.as_ref().and_then(|w| w.poll()) {
            self.show_completion(app, ready);
            changed = true;
        }
        for _ in 0..4 {
            let Some(result) = self.preparation.as_mut().and_then(|w| w.poll()) else {
                break;
            };
            changed |= self.prepared(app, result);
            crate::background::wake();
        }
        self.preview_completion(app);
        changed
    }
    fn show_completion(&mut self, app: &mut App, ready: CompletionReady) {
        let CompletionReady {
            pending: p,
            server,
            encoding,
            prefix,
            needle,
            rows,
        } = ready;
        if self
            .servers
            .get(&server)
            .is_none_or(|s| s.status != "Ready")
            || app.workspace.active_buffer() != p.buffer
            || !self.contextual_insert(app)
            || app.workspace.editor().buffer.revision != p.revision
            || app.workspace.editor().document_id != p.document
            || app.workspace.editor().buffer.cursor != p.cursor
        {
            return;
        }
        if !rows.is_empty() {
            let old = self
                .panel
                .as_ref()
                .filter(|p| p.kind == PanelKind::Completion);
            let selected_label =
                old.and_then(|p| p.visible().get(p.selected).map(|r| r.label.clone()));
            let size = old.map_or((0, 0), |p| p.popup_size);
            let selected = selected_label
                .and_then(|label| rows.iter().position(|r| r.label == label))
                .unwrap_or(0);
            self.completion_preview = None;
            self.completion_start = Some(prefix.start);
            self.completion_document = Some(p.document);
            self.completion_text = Some(app.workspace.editor().buffer.text.clone());
            self.completion_context = Some((p.buffer, p.revision, p.cursor, server, encoding));
            let mut panel = Panel::list("COMPLETION", PanelKind::Completion, rows);
            panel.popup_size = size;
            panel.selected = selected;
            panel.query = needle;
            self.panel = Some(panel);
            #[cfg(feature = "perf")]
            crate::perf::trace(
                "completion_ui_updated",
                format!("{}@{}", p.buffer, p.revision),
            );
            if self.completion_accept.take() == Some((p.buffer, p.revision, p.cursor)) {
                self.choose(app);
            }
        } else {
            if self
                .panel
                .as_ref()
                .is_some_and(|p| p.kind == PanelKind::Completion)
            {
                self.clear_panel();
            }
            self.completion_accept = None;
        }
    }
    fn preview_completion(&mut self, app: &mut App) {
        let Some(panel) = &self.panel else { return };
        if panel.kind != PanelKind::Completion {
            return;
        }
        let Some((buffer, revision, _, server, _)) = &self.completion_context else {
            return;
        };
        if *revision != app.workspace.editor().buffer.revision {
            return;
        }
        let stamp = (*buffer, *revision, panel.selected);
        if self.completion_preview == Some(stamp) {
            return;
        }
        self.completion_preview = Some(stamp);
        if self
            .servers
            .get(server)
            .is_none_or(|s| s.capabilities["completionProvider"]["resolveProvider"] != true)
        {
            return;
        }
        if let Some(Row {
            choice: Choice::Completion(value),
            ..
        }) = panel.visible().get(panel.selected).copied()
            && value["documentation"].is_null()
        {
            let value = value.clone();
            self.request(app, RequestKind::ResolvePreview, Some(value));
        }
    }
    fn event(&mut self, app: &mut App, server: String, event: lsp::Event) {
        match event {
            lsp::Event::Ready {
                capabilities,
                encoding,
            } => {
                let s = self.servers.get_mut(&server).unwrap();
                s.capabilities = capabilities;
                s.encoding = encoding;
                s.status = "Ready".into();
                self.completion_at = None;
                self.signature_seen = None;
            }
            lsp::Event::Stopped(error) => {
                self.signature = None;
                if self
                    .completion_context
                    .as_ref()
                    .is_some_and(|(_, _, _, name, _)| name == &server)
                {
                    if self
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.kind == PanelKind::Completion)
                    {
                        self.clear_panel();
                    }
                    self.completion_accept = None;
                }
                if let Some(s) = self.servers.get_mut(&server) {
                    s.status = error.clone();
                }
                self.message = format!("{server}: {error}");
                self.pending.retain(|_, p| p.server != server);
            }
            lsp::Event::Notification { method, params } => {
                if method == "textDocument/publishDiagnostics" {
                    if let Some(uri) = params["uri"].as_str()
                        && let Ok(path) = lsp::path(uri)
                    {
                        let key = lsp::path_key(&path);
                        let revision = app
                            .workspace
                            .buffers
                            .values()
                            .find(|e| {
                                e.buffer
                                    .path
                                    .as_ref()
                                    .is_some_and(|p| lsp::path_key(p) == key)
                            })
                            .map(|e| e.buffer.revision)
                            .unwrap_or(0);
                        if params["version"].as_u64().is_some_and(|v| v != revision) {
                            self.retire_lsp(params);
                            return;
                        }
                        let encoding = self.servers[&server].encoding;
                        let document = app.workspace.buffers.iter().find_map(|(id, e)| {
                            e.buffer
                                .path
                                .as_ref()
                                .filter(|p| lsp::path_key(p) == key)
                                .map(|_| (*id, e.document_id))
                        });
                        self.submit_preparation(
                            PreparationKey::Diagnostics(server.clone(), key.clone()),
                            PreparationJob {
                                server,
                                epoch: self.preparation_epoch,
                                encoding,
                                value: params,
                                context: PreparationContext::Diagnostics {
                                    key,
                                    revision,
                                    document,
                                },
                            },
                        );
                    }
                } else if method == "window/logMessage" || method == "window/showMessage" {
                    self.message = params["message"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(1000)
                        .collect();
                }
            }
            lsp::Event::ServerRequest { id, method, params } => {
                if method == "workspace/applyEdit" {
                    self.prepare_edit(
                        app,
                        params["edit"].clone(),
                        self.servers[&server].encoding,
                        Some((server, id)),
                    );
                } else {
                    self.servers[&server].client.respond(id, Value::Null);
                }
            }
            lsp::Event::Reply { id, result, error } => {
                let Some(p) = self.pending.remove(&id) else {
                    self.retire_lsp(result);
                    return;
                };
                if app
                    .workspace
                    .buffers
                    .get(&p.buffer)
                    .is_none_or(|e| e.buffer.revision != p.revision || e.document_id != p.document)
                {
                    self.retire_lsp(result);
                    return;
                }
                if let Some(error) = error {
                    if !matches!(
                        p.kind,
                        RequestKind::Completion | RequestKind::ResolvePreview
                    ) {
                        self.error(app, error)
                    }
                    return;
                }
                let encoding = self.servers[&server].encoding;
                if matches!(
                    p.kind,
                    RequestKind::Definition
                        | RequestKind::Implementation
                        | RequestKind::References
                        | RequestKind::Actions
                ) {
                    self.submit_preparation(
                        PreparationKey::Response(server.clone(), p.buffer, p.kind as u8),
                        PreparationJob {
                            server,
                            epoch: self.preparation_epoch,
                            encoding,
                            value: result,
                            context: PreparationContext::Response { id, pending: p },
                        },
                    );
                    return;
                }
                match p.kind {
                    RequestKind::Completion => {
                        if app.workspace.active_buffer() != p.buffer
                            || !self.contextual_insert(app)
                            || app.workspace.editor().buffer.cursor != p.cursor
                        {
                            return;
                        }
                        let prefix = word_range(&app.workspace.editor().buffer.text, p.cursor);
                        let needle = app
                            .workspace
                            .editor()
                            .buffer
                            .text
                            .slice(prefix.start..p.cursor)
                            .to_string()
                            .to_lowercase();
                        let worker = self.completion_worker.get_or_insert_with(|| {
                            crate::background::LatestWorker::new(|job: CompletionJob, cancel| {
                                let rows = if cancel.cancelled() {
                                    Vec::new()
                                } else {
                                    completion_rows_cancellable(job.result, &job.needle, || {
                                        cancel.cancelled()
                                    })
                                };
                                #[cfg(feature = "perf")]
                                crate::perf::trace(
                                    "completion_prepared",
                                    format!(
                                        "{}:{}@{}",
                                        job.server, job.pending.buffer, job.pending.revision
                                    ),
                                );
                                CompletionReady {
                                    pending: job.pending,
                                    server: job.server,
                                    encoding: job.encoding,
                                    prefix: job.prefix,
                                    needle: job.needle,
                                    rows,
                                }
                            })
                        });
                        worker.submit(CompletionJob {
                            result,
                            needle,
                            pending: p,
                            server,
                            encoding,
                            prefix,
                        });
                    }
                    RequestKind::Resolve => self.accept_completion(app, result),
                    RequestKind::ResolvePreview => {
                        if let Some(panel) = &mut self.panel
                            && panel.kind == PanelKind::Completion
                            && let Some(row) = panel
                                .rows
                                .iter_mut()
                                .find(|r| r.label == result["label"].as_str().unwrap_or(""))
                        {
                            row.detail = crate::ide_ui::clean(&format!(
                                "{}\n{}",
                                result["detail"].as_str().unwrap_or(""),
                                documentation(&result["documentation"])
                            ));
                            row.choice = Choice::Completion(result);
                            panel.invalidate();
                        }
                    }
                    RequestKind::Hover => {
                        let contents = if let Some(v) = result.get("contents") {
                            documentation(v)
                        } else {
                            String::new()
                        };
                        self.text(
                            "HOVER",
                            if contents.is_empty() {
                                "No hover information".into()
                            } else {
                                contents
                            },
                        );
                    }
                    RequestKind::Signature => {
                        if app.workspace.active_buffer() != p.buffer
                            || app.workspace.editor().buffer.cursor != p.cursor
                            || app.workspace.editor().mode != p.mode
                        {
                            return;
                        }
                        self.signature = serde_json::from_value::<lsp_types::SignatureHelp>(result)
                            .ok()
                            .filter(|h| !h.signatures.is_empty())
                            .map(|help| crate::contextual::Signature {
                                help,
                                buffer: p.buffer,
                                document: p.document,
                                call: crate::contextual::call_site(
                                    &app.workspace.editor().buffer.text,
                                    p.cursor,
                                ),
                            });
                    }
                    RequestKind::Definition
                    | RequestKind::Implementation
                    | RequestKind::References => unreachable!(),
                    RequestKind::Rename => self.prepare_edit(app, result, encoding, None),
                    RequestKind::Format | RequestKind::RangeFormat => {
                        let uri = lsp::uri(
                            app.workspace.buffers[&p.buffer]
                                .buffer
                                .path
                                .as_deref()
                                .unwrap(),
                        )
                        .unwrap();
                        self.prepare_edit(app, json!({"changes":{uri:result}}), encoding, None);
                    }
                    RequestKind::Actions => unreachable!(),
                    RequestKind::ResolveAction => self.apply_action(app, server, result),
                    RequestKind::Execute => {}
                }
            }
        }
    }
}
pub fn documentation(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(documentation).collect::<Vec<_>>().join("\n\n"),
        Value::Object(_) => value["value"].as_str().unwrap_or("").into(),
        _ => String::new(),
    }
}
fn word_range(text: &ropey::Rope, cursor: usize) -> Range<usize> {
    let mut start = cursor;
    while start > 0 {
        let c = text.char(start - 1);
        if c.is_alphanumeric() || c == '_' {
            start -= 1
        } else {
            break;
        }
    }
    start..cursor
}
pub fn apply_plan(app: &mut App, plan: Vec<lsp::PlannedDocument>) -> Result<()> {
    // Validate the entire transaction before touching any buffer.
    for d in &plan {
        if let Some((_, e)) = app.workspace.buffers.iter().find(|(_, e)| {
            e.buffer
                .path
                .as_ref()
                .is_some_and(|p| lsp::path_key(p) == lsp::path_key(&d.path))
        }) {
            if d.revision != Some(e.buffer.revision) {
                bail!("Workspace edit became stale; no files changed")
            }
        } else if d.revision.is_some() {
            bail!("Edited buffer was closed; no files changed")
        }
    }
    for d in plan {
        let id = app
            .workspace
            .buffers
            .iter()
            .find(|(_, e)| {
                e.buffer
                    .path
                    .as_ref()
                    .is_some_and(|p| lsp::path_key(p) == lsp::path_key(&d.path))
            })
            .map(|(id, _)| *id)
            .unwrap_or_else(|| {
                app.workspace
                    .insert_buffer(crate::buffer::Buffer::new(d.before, Some(d.path)))
            });
        app.workspace.external_edits(id, &d.edits);
    }
    app.notify(
        "Workspace edits applied to buffers; review and save each changed file",
        false,
    );
    Ok(())
}

/// Build the bounded popup candidate list from an owned protocol result.
#[doc(hidden)]
pub fn completion_rows(result: Value, needle: &str) -> Vec<Row> {
    completion_rows_cancellable(result, needle, || false)
}
fn completion_rows_cancellable(
    mut result: Value,
    needle: &str,
    cancelled: impl Fn() -> bool,
) -> Vec<Row> {
    let timer = crate::perf::Timer::start();
    let mut items = if result.is_array() {
        result
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    } else {
        result
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .unwrap_or_default()
    };
    let mut candidates: Vec<_> = items
        .iter()
        .enumerate()
        .take_while(|(index, _)| *index % 64 != 0 || !cancelled())
        .filter_map(|(index, value)| {
            let label = value["label"].as_str().unwrap_or("");
            let filter = value["filterText"].as_str().unwrap_or(label).to_lowercase();
            filter
                .contains(needle)
                .then(|| (!label.to_lowercase().starts_with(needle), index))
        })
        .collect();
    if cancelled() {
        return Vec::new();
    }
    let compare = |a: &(bool, usize), b: &(bool, usize)| {
        let key = |i: usize| {
            items[i]["sortText"]
                .as_str()
                .or(items[i]["label"].as_str())
                .unwrap_or("")
        };
        a.0.cmp(&b.0)
            .then_with(|| key(a.1).cmp(key(b.1)))
            .then(a.1.cmp(&b.1))
    };
    if candidates.len() > 200 {
        candidates.select_nth_unstable_by(200, compare);
        candidates.truncate(200);
    }
    candidates.sort_unstable_by(compare);
    let rows = candidates
        .into_iter()
        .map(|(_, index)| {
            let value = items[index].take();
            Row {
                label: value["label"].as_str().unwrap_or("").into(),
                detail: crate::ide_ui::clean(&format!(
                    "{}\n{}",
                    value["detail"].as_str().unwrap_or(""),
                    documentation(&value["documentation"])
                )),
                choice: Choice::Completion(value),
            }
        })
        .collect();
    timer.finish("completion_preparation");
    rows
}

/// Reuse a displayed item only across identifier edits inside its original word.
/// Extra edits and server commands may depend on arbitrary old document state;
/// those deliberately retain the fresh-response path.
fn rebase_completion(
    value: &Value,
    before: &ropey::Rope,
    buffer: &crate::buffer::Buffer,
    revision: u64,
    cursor: usize,
    encoding: Encoding,
) -> Option<Value> {
    if value.get("additionalTextEdits").is_some()
        || value.get("command").is_some()
        || value.get("data").is_some()
    {
        return None;
    }
    let start = word_range(before, cursor).start;
    let changes = buffer.changes_since(revision)?;
    if changes.is_empty() {
        return None;
    }
    let mut caret = cursor;
    for change in &changes {
        if change.range.start < start
            || change.range.end > caret
            || !change.text.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return None;
        }
        caret = caret.checked_add_signed(
            change.text.chars().count() as isize - change.range.len() as isize,
        )?;
    }
    if caret != buffer.cursor || word_range(&buffer.text, caret).start != start {
        return None;
    }
    let mut result = value.clone();
    if let Some(edit) = result.get_mut("textEdit") {
        for key in ["range", "insert", "replace"] {
            if let Some(range) = edit.get_mut(key) {
                let original: lsp_types::Range = serde_json::from_value(range.clone()).ok()?;
                let a = lsp::offset(before, original.start, encoding).ok()?;
                let mut b = lsp::offset(before, original.end, encoding).ok()?;
                // Only a replacement spanning the original typed prefix can be
                // safely extended; an unrelated/suffix-only edit is refreshed.
                if a != start || b < cursor {
                    return None;
                }
                for change in &changes {
                    if change.range.end > b {
                        return None;
                    }
                    b = b.checked_add_signed(
                        change.text.chars().count() as isize - change.range.len() as isize,
                    )?;
                }
                *range = json!(lsp::selection_range(&buffer.text, a..b, encoding));
            }
        }
    }
    Some(result)
}

#[cfg(test)]
mod font_ui_tests {
    use super::*;
    fn app(root: &std::path::Path) -> App {
        let mut config = crate::config::Config::default();
        config.git.enabled = false;
        config.lsp.enabled = false;
        App::with_history_path(
            crate::workspace::Workspace::new(crate::editor::Editor::new(config), root.into()),
            crate::storage::History::default(),
            root.join("history"),
        )
    }
    fn report(available: bool, managed: bool) -> font::Report {
        font::Report {
            summary: "Font: fixture\nInstallation: test fixture\nNo actual system changes".into(),
            targets: vec![font::terminal::Target {
                path: PathBuf::from("fixture-settings.json"),
                scope: font::terminal::Scope::Defaults,
                label: "Defaults".into(),
                current: Some("Old Font".into()),
            }],
            available,
            managed,
            installed: true,
        }
    }
    #[test]
    fn font_panel_uses_existing_navigation_and_never_installs_without_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.ide.font_panel(report(true, true), false, false);
        assert_eq!(app.ide.busy, 0);
        app.act(Action::Down);
        app.act(Action::Submit);
        assert!(matches!(
            app.ide.prompt.as_ref().unwrap().action,
            PromptAction::Font(font::Operation::Install)
        ));
        assert_eq!(app.ide.busy, 0);
        app.act(Action::Normal);
        assert!(app.ide.prompt.is_none());
        assert_eq!(app.ide.busy, 0);
        app.act(Action::PageDown);
        assert_eq!(app.ide.panel.as_ref().unwrap().scroll, 8);
        for (w, h) in [(110, 30), (35, 12), (12, 6), (8, 4)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| crate::ide_ui::draw(f, &mut app, &crate::theme::Theme::default()))
                .unwrap();
        }
        app.act(Action::Normal);
        assert!(app.ide.panel.is_none());
        assert_eq!(app.ide.busy, 0);
    }
    #[test]
    fn font_status_is_readonly_and_removal_requires_verified_ownership() {
        let mut ide = Ide::default();
        ide.font_panel(report(false, false), false, true);
        assert!(
            ide.panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .all(|r| matches!(r.choice, Choice::None | Choice::Cancel))
        );
        ide.font_panel(report(false, false), true, false);
        assert!(
            !ide.panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| matches!(r.choice, Choice::Font(_)))
        );
        ide.font_panel(report(false, true), true, false);
        assert!(
            ide.panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| matches!(r.choice, Choice::Font(font::Operation::Remove)))
        );
    }
}
#[cfg(test)]
mod performance_tests {
    use super::*;
    #[test]
    fn dismissed_completion_and_closed_document_metadata_release() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.git.enabled = false;
        config.lsp.enabled = false;
        let mut editor = crate::editor::Editor::new(config);
        editor.set_buffer(crate::buffer::Buffer::new(
            ropey::Rope::from_str(&"x".repeat(1_000_000)),
            None,
        ));
        let mut app = App::with_history_path(
            crate::workspace::Workspace::new(editor, dir.path().into()),
            crate::storage::History::default(),
            dir.path().join("history"),
        );
        let id = app.workspace.active_buffer();
        let mut ide = Ide {
            root: Some(app.workspace.root.clone()),
            completion_text: Some(app.workspace.editor().buffer.text.clone()),
            ..Default::default()
        };
        ide.changes.insert(id, (0, Instant::now()));
        app.workspace.close_buffer(id, true).unwrap();
        ide.tick(&mut app);
        assert!(ide.completion_text.is_none());
        assert!(!ide.changes.contains_key(&id));
    }
    #[test]
    fn completion_selection_matches_stable_reference_order() {
        let mut items: Vec<_> = (0..2000).map(|i| json!({"label":format!("{}value{i}", if i%3==0 {"pre"} else {""}), "sortText":format!("{}",i%27)})).collect();
        items.push(json!({"label":"unrelated"}));
        let mut reference: Vec<_> = items
            .iter()
            .filter(|v| {
                v["label"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
                    .contains("value")
            })
            .collect();
        reference.sort_by_key(|v| {
            (
                !v["label"].as_str().unwrap().starts_with("value"),
                v["sortText"]
                    .as_str()
                    .unwrap_or(v["label"].as_str().unwrap())
                    .to_owned(),
            )
        });
        let expected: Vec<_> = reference
            .into_iter()
            .take(200)
            .map(|v| v["label"].as_str().unwrap().to_owned())
            .collect();
        let actual = completion_rows(json!({"items":items}), "value");
        assert_eq!(
            actual.into_iter().map(|r| r.label).collect::<Vec<_>>(),
            expected
        );
    }
    #[test]
    fn completion_rebase_extends_unicode_edits_and_rejects_unsafe_changes() {
        let mut b = crate::buffer::Buffer::new(ropey::Rope::from_str("😀 va"), None);
        b.cursor = b.text.len_chars();
        let before = b.text.clone();
        let cursor = b.cursor;
        let value = json!({"label":"variable","insertText":"${1:variable}$0","insertTextFormat":2,"textEdit":{"range":lsp::selection_range(&before,2..cursor,Encoding::Utf16),"newText":"${1:variable}$0"}});
        b.insert("r");
        let rebased = rebase_completion(&value, &before, &b, 0, cursor, Encoding::Utf16).unwrap();
        let range: lsp_types::Range =
            serde_json::from_value(rebased["textEdit"]["range"].clone()).unwrap();
        assert_eq!(
            lsp::offset(&b.text, range.end, Encoding::Utf16).unwrap(),
            b.cursor
        );
        let mut extra = value.clone();
        extra["additionalTextEdits"] = json!([]);
        assert!(rebase_completion(&extra, &before, &b, 0, cursor, Encoding::Utf16).is_none());
        b.replace(0..0, "x");
        assert!(rebase_completion(&value, &before, &b, 0, cursor, Encoding::Utf16).is_none());
    }
}
