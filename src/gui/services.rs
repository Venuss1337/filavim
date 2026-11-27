//! Document-aware coordination of syntax, LSP runtimes and native popup state.
//!
//! [`Services::update`] schedules/synchronizes work; [`Services::poll`] drains
//! results on the UI thread with bounded server-event collection. Expensive
//! response preparation shares a keyed worker. Document/revision/request identity
//! is checked before publication; completion additionally owns a reusable typing
//! session in [`super::completion`]. GPUI drawing stays in the native view.
use super::documents::Documents;
use crate::{
    background::{self, KeyedWorker},
    buffer::Buffer,
    lsp::{self, Encoding},
    syntax::{self, Highlight, SyntaxService},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::Ordering,
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Completion,
    Resolve,
    Hover,
    Definition,
    Declaration,
    TypeDefinition,
    References,
    Implementation,
    Signature,
    Formatting,
    RangeFormatting,
    DocumentSymbols,
    WorkspaceSymbols,
    Rename,
    Actions,
    ActionResolve,
    Execute,
}
impl Kind {
    pub fn method(self) -> &'static str {
        match self {
            Self::Completion => "textDocument/completion",
            Self::Resolve => "completionItem/resolve",
            Self::Hover => "textDocument/hover",
            Self::Definition => "textDocument/definition",
            Self::Declaration => "textDocument/declaration",
            Self::TypeDefinition => "textDocument/typeDefinition",
            Self::References => "textDocument/references",
            Self::Implementation => "textDocument/implementation",
            Self::Signature => "textDocument/signatureHelp",
            Self::Formatting => "textDocument/formatting",
            Self::RangeFormatting => "textDocument/rangeFormatting",
            Self::DocumentSymbols => "textDocument/documentSymbol",
            Self::WorkspaceSymbols => "workspace/symbol",
            Self::Rename => "textDocument/rename",
            Self::Actions => "textDocument/codeAction",
            Self::ActionResolve => "codeAction/resolve",
            Self::Execute => "workspace/executeCommand",
        }
    }
}
#[derive(Clone)]
pub struct Diagnostic {
    pub range: lsp_types::Range,
    pub severity: u32,
    pub message: String,
}
#[derive(Clone)]
pub enum Choice {
    Text,
    Signature(Vec<std::ops::Range<usize>>),
    Buffer(u64),
    Command(String),
    Location {
        path: PathBuf,
        position: lsp_types::Position,
        encoding: Encoding,
    },
    Position(lsp_types::Position, Encoding),
    Completion(Value),
    Edit(Value),
    Action(Value),
}
#[derive(Clone)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub choice: Choice,
}
#[derive(Clone)]
pub struct Panel {
    pub title: String,
    pub request: u64,
    pub server: Option<String>,
    pub presentation: super::popup::Presentation,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub document: u64,
    pub revision: u64,
    pub cursor: usize,
    pub encoding: Encoding,
    pub kind: Kind,
    pub mode: crate::editor::Mode,
}
struct Server {
    client: lsp::Client,
    ready: bool,
    encoding: Encoding,
    documents: BTreeMap<u64, (String, u64)>,
    stopped: bool,
    resolve: bool,
    save: Option<bool>,
    id: String,
    root: PathBuf,
    config: lsp::ServerConfig,
    started: std::time::Instant,
    startup: Option<Duration>,
    capabilities: Value,
    failure: Option<String>,
}
#[derive(Clone)]
struct Pending {
    id: u64,
    server: String,
    document: u64,
    revision: u64,
    cursor: usize,
    kind: Kind,
    encoding: Encoding,
    candidate: Option<String>,
    #[cfg(feature = "perf")]
    started: std::time::Instant,
    #[cfg(feature = "perf")]
    prepared: std::time::Instant,
    source_revision: u64,
    snapshot: ropey::Rope,
    session: u64,
    incomplete: bool,
    parent: u64,
    selected: usize,
    mode: crate::editor::Mode,
}
pub struct WorkspaceApproval {
    pub token: u64,
    pub summaries: Vec<String>,
    pub reasons: Vec<String>,
    plan: lsp::workspace::Plan,
    context: WorkspaceContext,
}
struct WorkspaceContext {
    root: PathBuf,
    token: u64,
    documents: Vec<lsp::workspace::OpenDocument>,
    reply: Option<(String, Value, std::time::Instant)>,
    command: Option<(String, Value)>,
    subscriptions: Vec<(String, std::time::Instant, Value)>,
}
struct FileNotifications {
    key: String,
    started: std::time::Instant,
    messages: Vec<(String, Value)>,
}

enum WorkspaceJob {
    Workspace {
        value: Value,
        root: PathBuf,
        context: WorkspaceContext,
        encoding: Encoding,
    },
    CommitWorkspace {
        plan: lsp::workspace::Plan,
        context: WorkspaceContext,
    },
}
enum Job {
    Diagnostics {
        epoch: u64,
        owner: Option<std::time::Instant>,
        server: String,
        document: u64,
        revision: u64,
        encoding: Encoding,
        value: Value,
    },
    Reply {
        pending: Pending,
        value: Value,
    },
}
enum Prepared {
    Workspace(
        WorkspaceContext,
        Result<lsp::workspace::Plan, lsp::workspace::Failure>,
    ),
    Committed(
        WorkspaceContext,
        lsp::workspace::Outcome,
        Vec<FileNotifications>,
    ),
    Diagnostics {
        epoch: u64,
        owner: Option<std::time::Instant>,
        server: String,
        document: u64,
        revision: u64,
        encoding: Encoding,
        items: Vec<Diagnostic>,
        truncated: bool,
    },
    Panel(Pending, Vec<Row>),
}
fn same_document_uri(a: &str, b: &str) -> bool {
    a == b
        || match (lsp::path(a), lsp::path(b)) {
            (Ok(a), Ok(b)) => lsp::path_key(&a) == lsp::path_key(&b),
            _ => false,
        }
}
fn clean(s: &str, limit: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(limit)
        .collect()
}
fn documentation(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return clean(s, 8192);
    }
    if let Some(a) = v.as_array() {
        return a
            .iter()
            .take(32)
            .map(documentation)
            .collect::<Vec<_>>()
            .join("\n");
    }
    clean(v["value"].as_str().unwrap_or(""), 8192)
}
fn signature_information(value: &Value) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let active = value["activeSignature"].as_u64().unwrap_or(0) as usize;
    for (index, signature) in value["signatures"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .enumerate()
    {
        if text.len() > 16_384 {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        let label = signature["label"].as_str().unwrap_or("");
        let shown = clean(label, 2048);
        let start = text.len();
        text.push_str(&shown);
        if index == active && shown == label {
            let parameter = signature["activeParameter"]
                .as_u64()
                .or(value["activeParameter"].as_u64())
                .unwrap_or(0) as usize;
            let parameter = &signature["parameters"][parameter]["label"];
            let byte_at = |wanted: u64| {
                let mut offset = 0u64;
                for (byte, c) in label.char_indices() {
                    if offset == wanted {
                        return Some(byte);
                    }
                    offset += c.len_utf16() as u64;
                }
                (offset == wanted).then_some(label.len())
            };
            let range = if let Some(name) = parameter.as_str() {
                label.find(name).map(|at| at..at + name.len())
            } else {
                parameter[0]
                    .as_u64()
                    .zip(parameter[1].as_u64())
                    .and_then(|(a, b)| byte_at(a).zip(byte_at(b)))
                    .filter(|(a, b)| a <= b)
                    .map(|(a, b)| a..b)
            };
            if let Some(range) = range {
                highlights.push(start + range.start..start + range.end);
            }
        }
        let doc = documentation(&signature["documentation"]);
        if !doc.is_empty() {
            text.push('\n');
            text.push_str(&doc);
        }
    }
    (text, highlights)
}
fn completion_row(mut value: Value) -> Row {
    if !value.is_object() {
        return Row {
            label: "Invalid completion response".into(),
            detail: String::new(),
            choice: Choice::Text,
        };
    }
    let label = clean(value["label"].as_str().unwrap_or("").trim(), 256);
    let kind = match value["kind"].as_u64() {
        Some(2) => "Method",
        Some(3) => "Function",
        Some(5) => "Field",
        Some(6) => "Variable",
        Some(7) => "Class",
        Some(8) => "Interface",
        Some(9) => "Module",
        Some(10) => "Property",
        Some(12) => "Value",
        Some(13) => "Enum",
        Some(14) => "Keyword",
        Some(15) => "Snippet",
        Some(21) => "Constant",
        Some(22) => "Struct",
        Some(25) => "Type",
        _ => "Symbol",
    };
    let signature = value["detail"]
        .as_str()
        .or(value["labelDetails"]["description"].as_str())
        .unwrap_or("");
    let detail = format!(
        "{}\n{}",
        clean(signature, 2048),
        documentation(&value["documentation"])
    )
    .trim()
    .to_string();
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| {
            matches!(
                key.as_str(),
                "label"
                    | "textEdit"
                    | "insertText"
                    | "insertTextFormat"
                    | "additionalTextEdits"
                    | "data"
                    | "kind"
                    | "filterText"
                    | "sortText"
                    | "preselect"
                    | "commitCharacters"
                    | "filavimProvider"
                    | "filavimCandidate"
                    | "filavimResolved"
            )
        });
    }
    let filter = value["filterText"]
        .as_str()
        .or(value["label"].as_str())
        .unwrap_or("");
    let lower = filter.to_lowercase();
    let initials: String = filter
        .chars()
        .filter(|c| c.is_uppercase())
        .flat_map(char::to_lowercase)
        .collect();
    use std::hash::{Hash, Hasher};
    let mut key = std::collections::hash_map::DefaultHasher::new();
    for field in ["label", "filterText", "sortText", "insertText"] {
        value[field].as_str().hash(&mut key);
    }
    value["textEdit"]["newText"].as_str().hash(&mut key);
    detail.hash(&mut key);
    value["filavimKey"] = Value::String(format!("{:016x}", key.finish()));
    value["filavimFilter"] = Value::String(lower);
    value["filavimInitials"] = Value::String(initials);
    Row {
        label: format!("{label}  · {kind}"),
        detail,
        choice: Choice::Completion(value),
    }
}
fn symbol_rows(value: &Value, encoding: Encoding, rows: &mut Vec<Row>, depth: usize) {
    if depth > 32 {
        return;
    }
    for symbol in value.as_array().into_iter().flatten() {
        if rows.len() >= 200 {
            break;
        }
        let location = &symbol["location"];
        let range = if location["range"].is_object() {
            &location["range"]
        } else {
            symbol.get("selectionRange").unwrap_or(&symbol["range"])
        };
        if let Ok(position) = serde_json::from_value(range["start"].clone()) {
            let choice = location["uri"]
                .as_str()
                .and_then(|u| lsp::path(u).ok())
                .map_or(Choice::Position(position, encoding), |path| {
                    Choice::Location {
                        path,
                        position,
                        encoding,
                    }
                });
            rows.push(Row {
                label: clean(symbol["name"].as_str().unwrap_or("Symbol"), 256),
                detail: clean(
                    symbol["detail"]
                        .as_str()
                        .or(symbol["containerName"].as_str())
                        .unwrap_or(""),
                    512,
                ),
                choice,
            });
        }
        symbol_rows(&symbol["children"], encoding, rows, depth + 1);
    }
}
fn prepare(job: Job, cancel: &background::Cancel) -> Prepared {
    match job {
        Job::Diagnostics {
            epoch,
            owner,
            server,
            document,
            revision,
            encoding,
            value,
        } => {
            let source = value.as_array();
            let truncated = source.is_some_and(|v| v.len() > 50_000);
            let mut items = Vec::new();
            for (i, v) in source.into_iter().flatten().take(50_000).enumerate() {
                if i % 64 == 0 && cancel.cancelled() {
                    break;
                }
                if let Ok(range) = serde_json::from_value(v["range"].clone()) {
                    items.push(Diagnostic {
                        range,
                        severity: v["severity"].as_u64().unwrap_or(3) as u32,
                        message: clean(v["message"].as_str().unwrap_or(""), 2048),
                    });
                }
            }
            items.sort_by_key(|d| (d.range.start.line, d.range.start.character));
            Prepared::Diagnostics {
                epoch,
                owner,
                server,
                document,
                revision,
                encoding,
                items,
                truncated,
            }
        }
        Job::Reply {
            mut pending,
            mut value,
        } => {
            let mut rows = Vec::new();
            match pending.kind {
                Kind::Completion => {
                    pending.incomplete = value["isIncomplete"].as_bool().unwrap_or(false);
                    let defaults = value["itemDefaults"].clone();
                    let mut items = if value.is_array() {
                        value.as_array_mut().map(std::mem::take)
                    } else {
                        value["items"].as_array_mut().map(std::mem::take)
                    }
                    .unwrap_or_default();
                    pending.incomplete |= items.len() > 20_000;
                    for item in items.iter_mut().take(20_000) {
                        if !item.is_object() || !item["label"].is_string() {
                            continue;
                        }
                        if cancel.cancelled() {
                            break;
                        }
                        for key in ["insertTextFormat", "commitCharacters", "data"] {
                            if item[key].is_null() && !defaults[key].is_null() {
                                item[key] = defaults[key].clone();
                            }
                        }
                        if item["textEdit"].is_null() && !defaults["editRange"].is_null() {
                            let text = item["textEditText"]
                                .as_str()
                                .or(item["insertText"].as_str())
                                .or(item["label"].as_str())
                                .unwrap_or("")
                                .to_owned();
                            item["textEdit"] = if defaults["editRange"]["start"].is_object() {
                                json!({"range":defaults["editRange"],"newText":text})
                            } else {
                                json!({"insert":defaults["editRange"]["insert"],"replace":defaults["editRange"]["replace"],"newText":text})
                            };
                        }
                        rows.push(completion_row(item.take()));
                    }
                }
                Kind::Resolve => {
                    rows.push(completion_row(value));
                }
                Kind::Definition
                | Kind::Declaration
                | Kind::TypeDefinition
                | Kind::References
                | Kind::Implementation => {
                    let values = if let Some(a) = value.as_array() {
                        a.clone()
                    } else {
                        vec![value]
                    };
                    for v in values.into_iter().take(10_000) {
                        if cancel.cancelled() {
                            break;
                        }
                        let uri = v["uri"].as_str().or(v["targetUri"].as_str());
                        let position = v
                            .get("range")
                            .or(v.get("targetSelectionRange"))
                            .and_then(|r| serde_json::from_value(r["start"].clone()).ok());
                        if let (Some(path), Some(position)) =
                            (uri.and_then(|s| lsp::path(s).ok()), position)
                        {
                            let position: lsp_types::Position = position;
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
                                    encoding: pending.encoding,
                                },
                            });
                        }
                    }
                }
                Kind::Actions => {
                    for v in value.as_array().into_iter().flatten().take(200) {
                        if !v.is_object() || !v["title"].is_string() {
                            continue;
                        }
                        rows.push(Row {
                            label: clean(v["title"].as_str().unwrap_or("Code action"), 256),
                            detail: clean(v["disabled"]["reason"].as_str().unwrap_or(""), 512),
                            choice: if v["disabled"].is_object() {
                                Choice::Text
                            } else {
                                Choice::Action(v.clone())
                            },
                        });
                    }
                }
                Kind::ActionResolve => {
                    if !value.is_object() {
                        rows.push(Row {
                            label: "Invalid code-action response; no changes applied".into(),
                            detail: String::new(),
                            choice: Choice::Text,
                        });
                        return Prepared::Panel(pending, rows);
                    }
                    let mut value = value;
                    value["filavimResolved"] = Value::Bool(true);
                    rows.push(Row {
                        label: clean(value["title"].as_str().unwrap_or("Apply action"), 256),
                        detail: String::new(),
                        choice: Choice::Action(value),
                    });
                }
                Kind::Execute => rows.push(Row {
                    label: "Server command completed".into(),
                    detail: String::new(),
                    choice: Choice::Text,
                }),
                Kind::DocumentSymbols | Kind::WorkspaceSymbols => {
                    symbol_rows(&value, pending.encoding, &mut rows, 0)
                }
                Kind::Formatting | Kind::RangeFormatting => {
                    rows.push(Row {
                        label: "Apply formatting".into(),
                        detail: String::new(),
                        choice: Choice::Edit(value),
                    });
                }
                Kind::Rename => {
                    rows.push(Row {
                        label: "Apply rename".into(),
                        detail: String::new(),
                        choice: Choice::Edit(value),
                    });
                }
                Kind::Hover | Kind::Signature => {
                    let signature = signature_information(&value);
                    let text = if pending.kind == Kind::Hover {
                        documentation(&value["contents"])
                    } else {
                        signature.0
                    };
                    rows.push(Row {
                        label: if text.is_empty() {
                            "No information".into()
                        } else {
                            text
                        },
                        detail: String::new(),
                        choice: if pending.kind == Kind::Signature {
                            Choice::Signature(signature.1)
                        } else {
                            Choice::Text
                        },
                    });
                }
            }
            #[cfg(feature = "perf")]
            {
                pending.prepared = std::time::Instant::now();
            }
            Prepared::Panel(pending, rows)
        }
    }
}
/// UI-side service state, including lazy servers, syntax snapshots and popup sessions.
/// Call `update` after logical changes and `poll` on worker wakes; neither waits
/// for a server reply. Installation state is owned separately by `manager`.
pub struct Services {
    workspace_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    workspace_progress: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    workspace_total: usize,
    workspace_task: Option<std::sync::mpsc::Receiver<Prepared>>,
    workspace_requests: BTreeSet<String>,
    workspace_keep: BTreeSet<String>,
    pub workspace_approval: Option<WorkspaceApproval>,
    pub workspace_executing: bool,
    workspace_planning: bool,
    pub file_busy: bool,
    pub workspace_changed: bool,
    pub notifications: std::collections::VecDeque<super::toast::Notification>,
    manager_notified: bool,
    pub root: PathBuf,
    pub manager: crate::lsp_manager::Manager,
    paused: BTreeSet<String>,
    retiring: BTreeMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    syntax: Option<SyntaxService>,
    pub syntax_spans: BTreeMap<u64, (u64, Vec<Highlight>)>,
    syntax_sent: BTreeMap<u64, (u64, u64, Option<PathBuf>)>,
    servers: BTreeMap<String, Server>,
    retired_servers: Vec<lsp::Client>,
    pending: BTreeMap<u64, Pending>,
    preparation: Option<KeyedWorker<String, Job, Prepared>>,
    diagnostic_epochs: BTreeMap<u64, u64>,
    pub diagnostics: BTreeMap<(String, u64), (u64, Encoding, Vec<Diagnostic>)>,
    pub panel: Option<Panel>,
    pub signature: Option<Panel>,
    serial: u64,
    turn: usize,
    pub epoch: u64,
    pub message: Option<String>,
    shutting_down: bool,
    manual_start: bool,
    completion: Option<super::completion::Session>,
    #[cfg(feature = "perf")]
    completion_input: Option<std::time::Instant>,
    accept_when_ready: Option<(u64, usize)>,
}
impl Services {
    pub fn new(root: PathBuf) -> Self {
        Self {
            workspace_requests: BTreeSet::new(),
            workspace_keep: BTreeSet::new(),
            workspace_cancel: Default::default(),
            workspace_progress: Default::default(),
            workspace_total: 0,
            workspace_task: None,
            workspace_approval: None,
            workspace_executing: false,
            workspace_planning: false,
            file_busy: false,
            workspace_changed: false,
            notifications: Default::default(),
            manager_notified: false,
            manager: crate::lsp_manager::Manager::new(root.clone()),
            paused: BTreeSet::new(),
            retiring: BTreeMap::new(),
            root,
            syntax: None,
            syntax_spans: BTreeMap::new(),
            syntax_sent: BTreeMap::new(),
            servers: BTreeMap::new(),
            retired_servers: Vec::new(),
            pending: BTreeMap::new(),
            preparation: None,
            diagnostic_epochs: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            panel: None,
            serial: 0,
            turn: 0,
            epoch: 0,
            message: None,
            shutting_down: false,
            manual_start: false,
            completion: None,
            signature: None,
            #[cfg(feature = "perf")]
            completion_input: None,
            accept_when_ready: None,
        }
    }
    fn submit(&mut self, key: String, job: Job) {
        self.preparation
            .get_or_insert_with(|| KeyedWorker::new(prepare))
            .submit(key, job);
    }
    fn retire<T: Send + 'static>(&mut self, value: T) {
        self.preparation
            .get_or_insert_with(|| KeyedWorker::new(prepare))
            .retire(value);
    }
    fn notification(&mut self, category: super::toast::Category, title: &str, message: &str) {
        // Bound undelivered notices independently of transport/event queue sizes.
        if self.notifications.len() == 16 {
            self.notifications.pop_front();
        }
        self.notifications
            .push_back(super::toast::Notification::new(category, title, message));
    }
    pub fn update(&mut self, docs: &Documents) {
        if self.shutting_down || self.workspace_executing {
            return;
        }
        let live: BTreeSet<_> = docs.order.iter().copied().collect();
        self.syntax_spans.retain(|id, _| live.contains(id));
        self.syntax_sent.retain(|id, _| live.contains(id));
        self.diagnostic_epochs.retain(|id, _| live.contains(id));
        let closed: Vec<_> = self
            .diagnostics
            .keys()
            .filter(|(_, id)| !live.contains(id))
            .cloned()
            .collect();
        for key in closed {
            if let Some(old) = self.diagnostics.remove(&key) {
                self.retire(old);
            }
        }
        if let Some(s) = &self.syntax {
            s.retain_documents(&live);
        }
        let e = &docs.editor;
        let id = e.document_id;
        if e.config.syntax && e.buffer.text.len_bytes() <= e.config.syntax_max_bytes {
            let old = self.syntax_sent.get(&id);
            if old.is_none_or(|(rev, _, path)| *rev != e.buffer.revision || *path != e.buffer.path)
            {
                let changes = old
                    .and_then(|(rev, _, _)| e.buffer.changes_since(*rev))
                    .unwrap_or_default();
                let generation = self
                    .syntax
                    .get_or_insert_with(SyntaxService::new)
                    .request_changes(
                        id,
                        e.buffer.revision,
                        e.buffer.text.clone(),
                        e.buffer.path.as_deref(),
                        changes,
                    );
                self.syntax_sent
                    .insert(id, (e.buffer.revision, generation, e.buffer.path.clone()));
            }
        } else {
            self.syntax_spans.remove(&id);
            self.syntax_sent.remove(&id);
        }
        let compatible = self.completion.as_ref().is_some_and(|session| {
            e.mode == session.mode && session.compatible(docs.id(), &e.buffer)
        });
        if compatible {
            if self
                .completion
                .as_ref()
                .is_some_and(|c| c.revision != e.buffer.revision)
            {
                self.accept_when_ready = None;
            }
            if let Some(session) = &mut self.completion {
                session.advance(&e.buffer);
            }
            if self
                .panel
                .as_ref()
                .is_none_or(|p| p.revision != e.buffer.revision || p.cursor != e.buffer.cursor)
            {
                self.publish_completion(docs);
            }
        } else if let Some(old) = self.completion.take() {
            self.retire(old);
            if self
                .panel
                .as_ref()
                .is_some_and(|p| p.kind == Kind::Completion)
            {
                self.panel = None;
            }
        }
        // Cancel obsolete requests before they can install a popup or an edit.
        let stale: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| {
                if p.kind == Kind::Completion {
                    return !self
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.id == p.session && c.document == docs.id());
                }
                if p.kind == Kind::Resolve
                    && self.completion.as_ref().is_none_or(|c| c.id != p.parent)
                {
                    return true;
                }
                docs.id() != p.document
                    || docs.get(p.document).is_none_or(|e| {
                        e.buffer.revision != p.revision
                            || e.buffer.cursor != p.cursor
                            || e.mode != p.mode
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(p) = self.pending.remove(&id)
                && let Some(s) = self.servers.get(&p.server)
            {
                s.client.cancel(id);
            }
        }
        if self.panel.as_ref().is_some_and(|p| {
            docs.get(p.document).is_none_or(|e| {
                e.buffer.revision != p.revision || e.mode != p.mode || e.buffer.cursor != p.cursor
            }) || docs.id() != p.document
        }) {
            self.panel = None;
        }
        if self.signature.as_ref().is_some_and(|p| {
            p.document != docs.id()
                || p.revision != e.buffer.revision
                || p.cursor != e.buffer.cursor
                || p.mode != e.mode
        }) {
            self.signature = None;
        }
        for server in self.servers.values_mut() {
            let closed: Vec<_> = server
                .documents
                .keys()
                .filter(|id| !live.contains(id))
                .copied()
                .collect();
            for id in closed {
                if let Some((uri, _)) = server.documents.remove(&id) {
                    server
                        .client
                        .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
                }
            }
        }
        if !e.config.lsp.enabled {
            if !self.servers.is_empty() {
                self.restart();
            }
            return;
        }
        if !e.config.lsp.auto_start && !self.manual_start {
            return;
        }
        let paths: BTreeSet<PathBuf> = docs
            .order
            .iter()
            .filter_map(|id| docs.get(*id).and_then(|e| e.buffer.path.clone()))
            .collect();
        // An empty initial editor does no tool discovery until the manager is opened.
        if paths.is_empty() && !self.manager.loaded {
            return;
        }
        // Resolution and root-marker scans happen on the manager worker, never here.
        self.manager.set_workspace(&self.root);
        self.manager.observe(paths);
        if !self.manager.has_snapshot {
            return;
        }
        self.retiring
            .retain(|_, stopped| !stopped.load(Ordering::Acquire));
        let mut desired = BTreeSet::new();
        let defaults = lsp::Config::default();
        let preferences = self.manager.preferences();
        for id in &docs.order {
            let e = docs.get(*id).unwrap();
            let Some(path) = &e.buffer.path else {
                continue;
            };
            let Some(language) = lsp::language(path) else {
                continue;
            };
            let explicit = e
                .config
                .lsp
                .servers
                .iter()
                .find(|s| s.enabled && s.languages.iter().any(|l| l == language));
            let custom = explicit.is_some_and(|s| !defaults.servers.iter().any(|d| d == s));
            let catalog_language = crate::lsp_catalog::primary(language).is_some();
            // Preserve legacy custom configurations until the user explicitly
            // chooses a catalog primary for this language and scope.
            let manager_primary_override = preferences.primary.contains_key(language);
            let configurations: Vec<crate::lsp_manager::Resolved> =
                if custom && !manager_primary_override || !catalog_language {
                    e.config
                        .lsp
                        .servers
                        .iter()
                        .filter(|s| s.enabled && s.languages.iter().any(|l| l == language))
                        .map(|s| crate::lsp_manager::Resolved {
                            id: s.name.clone(),
                            root: self.root.clone(),
                            config: s.clone(),
                        })
                        .collect()
                } else {
                    self.manager
                        .snapshot
                        .resolved
                        .get(&lsp::path_key(path))
                        .map(|items| {
                            items
                                .iter()
                                .filter(|item| {
                                    explicit.is_some() || preferences.enabled.contains(&item.id)
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                };
            for resolved in configurations {
                if preferences.all_disabled == Some(true) {
                    continue;
                }
                let config = resolved.config;
                let key = format!("{}@{}", config.name, lsp::path_key(&resolved.root));
                desired.insert(key.clone());
                if self.paused.contains(&resolved.id) || self.retiring.contains_key(&key) {
                    continue;
                }
                if self.servers.get(&key).is_some_and(|s| s.config != config) {
                    self.stop_instance(&key);
                    continue;
                }
                let server = self.servers.entry(key).or_insert_with(|| Server {
                    client: lsp::Client::start(
                        config.clone(),
                        resolved.root.clone(),
                        Duration::from_millis(e.config.lsp.timeout_ms),
                    ),
                    ready: false,
                    encoding: Encoding::Utf16,
                    documents: BTreeMap::new(),
                    stopped: false,
                    resolve: false,
                    save: None,
                    id: resolved.id,
                    root: resolved.root,
                    config,
                    started: std::time::Instant::now(),
                    startup: None,
                    capabilities: Value::Null,
                    failure: None,
                });
                if server.stopped {
                    continue;
                }
                let Ok(uri) = lsp::uri(path) else {
                    continue;
                };
                let old = server.documents.get(id);
                if old.is_some_and(|(u, rev)| *u == uri && *rev == e.buffer.revision) {
                    continue;
                }
                let old = old.cloned();
                if let Some((old_uri, _)) = &old
                    && *old_uri != uri
                {
                    server.client.notify(
                        "textDocument/didClose",
                        json!({"textDocument":{"uri":old_uri}}),
                    );
                }
                let open = old.as_ref().is_none_or(|(u, _)| *u != uri);
                let changes = old
                    .as_ref()
                    .filter(|(u, _)| *u == uri)
                    .and_then(|(_, rev)| e.buffer.changes_since(*rev).map(|c| (*rev, c)));
                server.client.document_changes(
                    uri.clone(),
                    language.into(),
                    e.buffer.revision,
                    e.buffer.text.clone(),
                    open,
                    changes,
                );
                server.documents.insert(*id, (uri, e.buffer.revision));
            }
        }
        // A path-resolution refresh must not retire a compatible process between
        // closing its old URI and opening the new one. Likewise, acknowledge a
        // server edit and queue its command before releasing its process lease.
        if self.manager.busy {
            desired.extend(self.servers.keys().cloned());
        }
        desired.extend(self.workspace_keep.iter().cloned());
        desired.extend(
            self.pending
                .values()
                .filter(|p| p.kind == Kind::Execute)
                .map(|p| p.server.clone()),
        );
        let obsolete: Vec<_> = self
            .servers
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect();
        for key in obsolete {
            self.stop_instance(&key);
        }
    }

    fn install_information(&mut self, kind: Kind, panel: Option<Panel>) {
        if kind == Kind::Signature
            && self
                .panel
                .as_ref()
                .is_some_and(|p| p.kind == Kind::Completion)
        {
            self.signature = panel;
        } else {
            self.signature = None;
            self.panel = panel;
        }
    }
    fn publish_completion(&mut self, docs: &Documents) {
        if self.completion.is_some()
            && self
                .panel
                .as_ref()
                .is_some_and(|p| p.kind == Kind::Signature)
        {
            self.signature = self.panel.take();
        }
        let Some(session) = &mut self.completion else {
            return;
        };
        if self
            .panel
            .as_ref()
            .is_some_and(|p| p.kind != Kind::Completion)
        {
            return;
        }
        let selected = self
            .panel
            .as_ref()
            .and_then(|p| p.rows.get(p.selected))
            .and_then(super::completion::identity)
            .map(str::to_owned);
        let selected_label = self
            .panel
            .as_ref()
            .and_then(|p| p.rows.get(p.selected))
            .and_then(|row| match &row.choice {
                Choice::Completion(v) => v["label"].as_str(),
                _ => None,
            })
            .map(str::to_owned);
        let mut rows = session.rows(&docs.editor.buffer, &docs.editor.config.completion.matching);
        session.lifecycle = if rows.is_empty() {
            super::completion::Lifecycle::Empty
        } else {
            super::completion::Lifecycle::Visible
        };
        if let Some(old) = self.panel.as_ref().and_then(|p| p.rows.get(p.selected))
            && let Some(current) = rows
                .iter_mut()
                .find(|r| super::completion::identity(r) == super::completion::identity(old))
            && !old.detail.is_empty()
        {
            current.detail = old.detail.clone();
        }
        if rows.is_empty() {
            self.panel = self.signature.take();
            return;
        }
        #[cfg(feature = "perf")]
        if !session.presented {
            crate::perf::record("completion.first_available", session.started.elapsed());
            session.presented = true;
        }

        let selected = selected
            .and_then(|id| {
                rows.iter()
                    .position(|r| super::completion::identity(r) == Some(id.as_str()))
            })
            .or_else(|| {
                selected_label.and_then(|label| {
                    rows.iter().position(
                        |row| matches!(&row.choice, Choice::Completion(v) if v["label"] == label),
                    )
                })
            })
            .unwrap_or(0);
        let provider = match &rows[selected].choice {
            Choice::Completion(v) => v["filavimProvider"].as_str(),
            _ => None,
        };
        let encoding = provider
            .and_then(|key| session.sources.get(key))
            .map_or(Encoding::Utf16, |s| s.encoding);
        self.panel = Some(Panel {
            title: "Completion".into(),
            request: session.id,
            server: provider.map(str::to_owned),
            presentation: super::popup::Presentation::Completion,
            rows,
            selected,
            document: docs.id(),
            revision: docs.editor.buffer.revision,
            cursor: docs.editor.buffer.cursor,
            encoding,
            kind: Kind::Completion,
            mode: docs.editor.mode,
        });
    }
    /// Publish reusable/local candidates before asynchronously refreshing providers.
    /// `manual` permits explicit invocation outside automatic Insert-mode triggers.
    pub fn request_completion(&mut self, docs: &Documents, manual: bool) {
        if self.workspace_active() {
            return;
        }
        #[cfg(feature = "perf")]
        let input = if manual {
            self.completion_input.take();
            None
        } else {
            self.completion_input.take()
        };
        self.manual_start = true;
        self.update(docs);
        let e = &docs.editor;
        if e.mode != crate::editor::Mode::Insert && !manual {
            return;
        }
        let Some(path) = &e.buffer.path else { return };
        if self.completion.is_none() {
            self.serial += 1;
            self.completion = Some(super::completion::Session::new(
                self.serial,
                docs.id(),
                &e.buffer,
                e.mode,
            ));
            #[cfg(feature = "perf")]
            if let Some(at) = input {
                self.completion
                    .as_mut()
                    .expect("session initialized")
                    .started = at;
            }
        }
        let local_key = super::completion::LOCAL_TYPES;
        if !self.completion.as_ref().is_some_and(|s| s.local_checked) {
            self.completion
                .as_mut()
                .expect("session initialized")
                .local_checked = true;
            let rows: Vec<_> = super::completion_context::keywords(&e.buffer)
                .iter()
                .map(|word| {
                    completion_row(json!({"label":word,"insertText":word,"kind":14,
                    "detail":"TypeScript type keyword · local","filavimResolved":true}))
                })
                .collect();
            if !rows.is_empty() {
                self.completion
                    .as_mut()
                    .expect("session initialized")
                    .sources
                    .insert(
                        local_key.into(),
                        super::completion::Source {
                            rows,
                            text: e.buffer.text.clone(),
                            revision: e.buffer.revision,
                            encoding: Encoding::Utf16,
                            incomplete: false,
                            query: String::new(),
                        },
                    );
                self.publish_completion(docs);
            }
        }
        if manual {
            let ids: Vec<_> = self
                .pending
                .iter()
                .filter(|(_, p)| p.kind == Kind::Completion)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(p) = self.pending.remove(&id)
                    && let Some(server) = self.servers.get(&p.server)
                {
                    server.client.cancel(id);
                }
            }
        }
        let session = self.completion.as_ref().expect("session initialized");
        let last = e
            .buffer
            .cursor
            .checked_sub(1)
            .map(|at| e.buffer.text.char(at).to_string());
        let mut requests = Vec::new();
        for (key, server) in &self.servers {
            if !server.ready
                || server.stopped
                || !server.documents.contains_key(&docs.id())
                || !supports(&server.capabilities, Kind::Completion)
            {
                continue;
            }
            // A slow valid request can still supply the narrowed query. Do not starve it by
            // cancelling on every keystroke, and never queue a second request behind it.
            if self
                .pending
                .values()
                .any(|p| p.kind == Kind::Completion && p.server == *key && p.session == session.id)
            {
                continue;
            }
            let source = session.sources.get(key);
            let visible = self.panel.as_ref().is_some_and(|panel| {
                panel.rows.iter().any(|row| {
                matches!(&row.choice, Choice::Completion(v) if v["filavimProvider"] == key.as_str())
            })
            });
            if !manual
                && source.is_some_and(|s| {
                    s.reusable(&e.buffer, &session.query)
                        && (s.revision == e.buffer.revision || visible)
                })
            {
                continue;
            }
            let trigger = last.as_ref().filter(|c| {
                server.capabilities["completionProvider"]["triggerCharacters"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(c)))
            });
            let context = if source.is_some_and(|s| s.incomplete) {
                json!({"triggerKind":3})
            } else if let Some(c) = trigger {
                json!({"triggerKind":2,"triggerCharacter":c})
            } else {
                json!({"triggerKind":1})
            };
            requests.push((key.clone(), server.encoding, context));
        }
        let session_id = session.id;
        for (key, encoding, context) in requests {
            self.serial += 1;
            let id = self.serial;
            let params = json!({"textDocument":{"uri":lsp::uri(path).unwrap_or_default()},"position":lsp::position(&e.buffer.text,e.buffer.cursor,encoding),"context":context});
            self.servers[&key]
                .client
                .request(id, Kind::Completion.method(), params);
            self.pending.insert(
                id,
                Pending {
                    id,
                    server: key,
                    document: docs.id(),
                    revision: e.buffer.revision,
                    cursor: e.buffer.cursor,
                    kind: Kind::Completion,
                    encoding,
                    source_revision: e.buffer.revision,
                    snapshot: e.buffer.text.clone(),
                    session: session_id,
                    incomplete: false,
                    candidate: None,
                    #[cfg(feature = "perf")]
                    started: std::time::Instant::now(),
                    #[cfg(feature = "perf")]
                    prepared: std::time::Instant::now(),
                    parent: 0,
                    selected: 0,
                    mode: e.mode,
                },
            );
        }
    }
    #[cfg(feature = "perf")]
    pub fn note_completion_input(&mut self, at: std::time::Instant) {
        self.completion_input = Some(at);
    }
    pub fn completion_query(&self) -> &str {
        self.completion.as_ref().map_or("", |c| c.query.as_str())
    }
    pub fn automatic_completion(&mut self, docs: &Documents) {
        let e = &docs.editor;
        if e.mode != crate::editor::Mode::Insert
            || !e.config.completion.enabled
            || !e.config.completion.auto_trigger
        {
            return;
        }
        let prefix = e.buffer.cursor - super::completion::prefix(&e.buffer);
        let last = e
            .buffer
            .cursor
            .checked_sub(1)
            .map(|at| e.buffer.text.char(at).to_string());
        let trigger = last.as_ref().is_some_and(|c| {
            self.servers.values().any(|s| {
                s.capabilities["completionProvider"]["triggerCharacters"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(c)))
            })
        });
        let type_position = prefix == 0
            && e.config.completion.minimum_prefix <= 1
            && !super::completion_context::keywords(&e.buffer).is_empty();
        if trigger || type_position || prefix >= e.config.completion.minimum_prefix.max(1) {
            self.request_completion(docs, false);
        }
    }
    pub fn request(&mut self, docs: &Documents, kind: Kind, mut extra: Option<Value>) {
        let preferred = extra
            .as_mut()
            .and_then(Value::as_object_mut)
            .and_then(|v| v.remove("filavimServer"))
            .and_then(|v| v.as_str().map(str::to_owned));
        if kind == Kind::Completion {
            self.request_completion(docs, extra.is_none());
            return;
        }
        self.manual_start = true;
        self.update(docs);
        let e = &docs.editor;
        let path = e.buffer.path.as_ref();
        if path.is_none() && kind != Kind::Execute {
            self.message = Some("Save this document before requesting language services".into());
            return;
        }
        let Some((key, server)) = self
            .servers
            .iter()
            .filter(|(key, s)| {
                preferred.as_ref().is_none_or(|p| p == *key)
                    && s.ready
                    && !s.stopped
                    && (kind == Kind::Execute && preferred.is_some()
                        || s.documents.contains_key(&e.document_id))
                    && supports(&s.capabilities, kind)
            })
            .min_by_key(|(_, s)| {
                if matches!(
                    kind,
                    Kind::Actions | Kind::ActionResolve | Kind::Execute | Kind::Formatting
                ) {
                    if s.id == "ruff" { 0 } else { 1 }
                } else if s.id == "ruff" {
                    1
                } else {
                    0
                }
            })
        else {
            self.message = Some(
                "Language server unavailable or initializing; use :LspRestart to retry".into(),
            );
            return;
        };
        let uri = path.and_then(|p| lsp::uri(p).ok()).unwrap_or_default();
        let position = lsp::position(&e.buffer.text, e.buffer.cursor, server.encoding);
        let mut params = json!({"textDocument":{"uri":uri},"position":position});
        if kind == Kind::References {
            params["context"] = json!({"includeDeclaration":true});
        }
        if matches!(kind, Kind::Formatting | Kind::RangeFormatting) {
            params = json!({"textDocument":{"uri":uri},"options":{"tabSize":e.config.tab_width,"insertSpaces":e.config.expand_tabs}});
        }
        if kind == Kind::RangeFormatting {
            let range = e.selection().unwrap_or(e.buffer.cursor..e.buffer.cursor);
            params["range"] = json!({"start":lsp::position(&e.buffer.text,range.start,server.encoding),"end":lsp::position(&e.buffer.text,range.end,server.encoding)});
        }
        if kind == Kind::WorkspaceSymbols {
            params = json!({"query":""});
        }
        if kind == Kind::Actions {
            params["range"] = json!({"start":position,"end":position});
            params["context"] = json!({"diagnostics":[]});
        }
        if matches!(kind, Kind::ActionResolve | Kind::Execute) {
            params = json!({});
        }
        if let Some(extra) = extra
            && let Some(object) = extra.as_object()
        {
            for (k, v) in object {
                params[k] = v.clone();
            }
        }
        if kind == Kind::Execute
            && !server.capabilities["executeCommandProvider"]["commands"]
                .as_array()
                .is_some_and(|commands| commands.iter().any(|c| c == &params["command"]))
        {
            self.message = Some("Server command was not advertised; nothing was executed".into());
            return;
        }
        let cancelled: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.document == docs.id() && p.kind == kind)
            .map(|(id, _)| *id)
            .collect();
        for id in cancelled {
            if let Some(p) = self.pending.remove(&id)
                && let Some(s) = self.servers.get(&p.server)
            {
                s.client.cancel(id);
            }
        }
        let (key, encoding) = (key.clone(), server.encoding);
        self.serial += 1;
        let id = self.serial;
        let mut start = e.buffer.cursor;
        if kind == Kind::Completion {
            while start > 0 {
                let ch = e.buffer.text.char(start - 1);
                if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                if e.buffer.cursor - start >= 256 {
                    self.message = Some("Completion prefix exceeds 256 characters".into());
                    return;
                }
                start -= 1;
            }
        }
        self.servers[&key].client.request(id, kind.method(), params);
        self.pending.insert(
            id,
            Pending {
                id,
                server: key,
                document: docs.id(),
                revision: e.buffer.revision,
                cursor: e.buffer.cursor,
                kind,
                encoding,
                source_revision: e.buffer.revision,
                snapshot: e.buffer.text.clone(),
                session: 0,
                incomplete: false,
                candidate: None,
                #[cfg(feature = "perf")]
                started: std::time::Instant::now(),
                #[cfg(feature = "perf")]
                prepared: std::time::Instant::now(),
                parent: 0,
                selected: 0,
                mode: e.mode,
            },
        );
    }
    /// Consume ready work and report whether visible state changed.
    /// Server collection rotates its starting point and yields after its budget;
    /// additional work wakes the view again instead of waiting for an idle timer.
    pub fn poll(&mut self, docs: &mut Documents) -> bool {
        for client in std::mem::take(&mut self.retired_servers) {
            if client.stopped.load(Ordering::Acquire) {
                self.retire(client);
            } else {
                self.retired_servers.push(client);
            }
        }
        let manager_was_busy = self.manager.busy;
        let operation = self.manager.operation.clone();
        if manager_was_busy && !self.manager_notified {
            self.manager_notified = true;
            if let Some(crate::lsp_manager::Operation::Install(id)) = &operation {
                self.notification(
                    super::toast::Category::Information,
                    "Language tools",
                    &format!("Installing / updating {id}"),
                );
            }
        }
        let mut refresh_completion = false;
        let mut changed = self.manager.poll();
        if manager_was_busy && !self.manager.busy {
            self.manager_notified = false;
            if matches!(
                &operation,
                Some(
                    crate::lsp_manager::Operation::Install(_)
                        | crate::lsp_manager::Operation::Remove(_)
                )
            ) {
                if let Some(error) = self.manager.error.clone() {
                    self.notification(super::toast::Category::Error, "Language tools", &error);
                } else {
                    let message = match &operation {
                        Some(crate::lsp_manager::Operation::Install(id)) => {
                            format!("{id} installation completed")
                        }
                        Some(crate::lsp_manager::Operation::Remove(id)) => {
                            format!("{id} managed installation removed")
                        }
                        _ => unreachable!(),
                    };
                    self.notification(super::toast::Category::Success, "Language tools", &message);
                }
            }
        }
        if manager_was_busy && !self.manager.busy && self.manager.error.is_none() {
            use crate::lsp_manager::{Change, Operation};
            match self.manager.operation.take() {
                Some(Operation::Install(id))
                | Some(Operation::Configure {
                    change: Change::Select(id),
                    ..
                })
                | Some(Operation::Configure {
                    change: Change::Enable(id, true),
                    ..
                }) => {
                    self.paused.remove(&id);
                }
                _ => {}
            }
        }
        if changed {
            self.update(docs);
        }
        if let Some(syntax) = &self.syntax {
            for result in syntax.results.try_iter().take(16) {
                if self
                    .syntax_sent
                    .get(&result.document)
                    .is_some_and(|(rev, generation, _)| {
                        *rev == result.revision && *generation == result.generation
                    })
                    && docs
                        .get(result.document)
                        .is_some_and(|e| e.buffer.revision == result.revision)
                {
                    self.syntax_spans
                        .insert(result.document, (result.revision, result.highlights));
                    self.epoch += 1;
                    changed = true;
                }
            }
        }
        let keys: Vec<_> = self.servers.keys().cloned().collect();
        let mut events = Vec::new();
        if !keys.is_empty() {
            let start = std::time::Instant::now();
            'drain: loop {
                let mut found = false;
                for i in 0..keys.len() {
                    let key = &keys[(i + self.turn) % keys.len()];
                    if let Ok(event) = self.servers[key].client.events.try_recv() {
                        events.push((key.clone(), event));
                        found = true;
                    }
                    if events.len() >= 32 || start.elapsed() >= Duration::from_millis(2) {
                        background::wake();
                        break 'drain;
                    }
                }
                if !found {
                    break;
                }
            }
            self.turn = (self.turn + 1) % keys.len();
        }
        for (key, event) in events {
            match event {
                lsp::Event::Ready {
                    encoding,
                    capabilities,
                } => {
                    let s = self.servers.get_mut(&key).unwrap();
                    s.ready = true;
                    s.client.notify(
                        "workspace/didChangeConfiguration",
                        json!({"settings":s.config.settings}),
                    );
                    s.startup = Some(s.started.elapsed());
                    s.capabilities = capabilities.clone();
                    let save = &capabilities["textDocumentSync"]["save"];
                    s.save = if save == true {
                        Some(false)
                    } else if save.is_object() {
                        Some(save["includeText"] == true)
                    } else {
                        None
                    };
                    s.encoding = encoding;
                    s.resolve = capabilities["completionProvider"]["resolveProvider"].as_bool()
                        == Some(true);
                    self.automatic_completion(docs);
                    self.notification(
                        super::toast::Category::Information,
                        "Language server",
                        &format!("{key} ready"),
                    );
                    changed = true;
                }
                lsp::Event::Stopped(message) => {
                    if self
                        .signature
                        .as_ref()
                        .is_some_and(|p| p.server.as_deref() == Some(&key))
                    {
                        self.signature = None;
                    }
                    let server = self.servers.get_mut(&key).unwrap();
                    server.stopped = true;
                    server.failure = Some(message.clone());
                    self.diagnostics.retain(|(server, _), _| server != &key);
                    self.pending.retain(|_, p| p.server != key);
                    if let Some(session) = &mut self.completion {
                        session.sources.remove(&key);
                    }
                    self.publish_completion(docs);
                    if !self.shutting_down {
                        self.notification(
                            super::toast::Category::Error,
                            "Language server",
                            &message,
                        );
                    }
                    changed = true;
                }
                lsp::Event::ServerRequest { id, method, params } => {
                    if method == "workspace/applyEdit" {
                        let encoding = self.servers[&key].encoding;
                        let started = self.servers[&key].started;
                        let request_key = format!("{key}:{started:?}:{id}");
                        // A repeated active request ID cannot redirect an approved
                        // plan. IDs may be reused legitimately after its reply.
                        if !self.workspace_requests.insert(request_key) {
                            continue;
                        }
                        self.workspace_plan(
                            docs,
                            params["edit"].clone(),
                            encoding,
                            Some((key.clone(), id, started)),
                            None,
                        );
                        continue;
                    }
                    if matches!(
                        method.as_str(),
                        "client/registerCapability" | "client/unregisterCapability"
                    ) {
                        let registrations = params
                            .get("registrations")
                            .or_else(|| params.get("unregisterations"))
                            .or_else(|| params.get("unregistrations"));
                        if registrations.and_then(Value::as_array).is_none_or(|items| {
                            items
                                .iter()
                                .any(|r| r["method"] != "textDocument/completion")
                        }) {
                            self.servers[&key].client.respond_error(
                                id,
                                -32602,
                                "Only dynamic completion registration is supported",
                            );
                            continue;
                        }
                    }
                    let result = match method.as_str() {
                        "client/registerCapability" => {
                            for registration in
                                params["registrations"].as_array().into_iter().flatten()
                            {
                                if registration["method"] == "textDocument/completion" {
                                    let server = self.servers.get_mut(&key).expect("live server");
                                    let options =
                                        registration["registerOptions"].as_object().map_or_else(
                                            || json!({}),
                                            |options| Value::Object(options.clone()),
                                        );
                                    server.resolve = options["resolveProvider"] == true;
                                    server.capabilities["completionProvider"] = options;
                                }
                            }
                            Value::Null
                        }
                        "client/unregisterCapability" => {
                            for registration in
                                params["unregisterations"].as_array().into_iter().flatten()
                            {
                                if registration["method"] == "textDocument/completion" {
                                    self.servers
                                        .get_mut(&key)
                                        .expect("live server")
                                        .capabilities["completionProvider"] = Value::Null;
                                    self.servers.get_mut(&key).expect("live server").resolve =
                                        false;
                                }
                            }
                            Value::Null
                        }
                        "workspace/applyEdit" => {
                            json!({"applied":false,"failureReason":"Use an explicit native code action or rename"})
                        }
                        "workspace/configuration" => {
                            lsp::configuration_values(&self.servers[&key].config.settings, &params)
                        }
                        "workspace/workspaceFolders" => {
                            json!([{"uri":lsp::uri(&self.servers[&key].root).unwrap_or_default(),"name":self.servers[&key].root.file_name().unwrap_or_default().to_string_lossy()}])
                        }
                        "window/showMessageRequest" => Value::Null,
                        _ => {
                            self.servers[&key].client.respond_error(
                                id,
                                -32601,
                                "Client method is unsupported",
                            );
                            continue;
                        }
                    };
                    self.servers[&key].client.respond(id, result);
                    if method == "client/registerCapability" {
                        // Some servers advertise completion only after initialization.
                        // Re-evaluate the current input when it becomes available.
                        self.automatic_completion(docs);
                    }
                }
                lsp::Event::Notification { method, mut params }
                    if method == "textDocument/publishDiagnostics" =>
                {
                    let uri = params["uri"].as_str().unwrap_or("");
                    let s = &self.servers[&key];
                    if let Some((&document, (_, revision))) = s
                        .documents
                        .iter()
                        .find(|(_, p)| same_document_uri(&p.0, uri))
                        && params["version"].as_u64().is_none_or(|v| v == *revision)
                    {
                        self.submit(
                            format!("d:{key}:{document}"),
                            Job::Diagnostics {
                                epoch: self.diagnostic_epochs.get(&document).copied().unwrap_or(0),
                                owner: Some(s.started),
                                server: key.clone(),
                                document,
                                revision: *revision,
                                encoding: s.encoding,
                                value: params["diagnostics"].take(),
                            },
                        );
                    }
                }
                lsp::Event::Reply { id, result, error } => {
                    if let Some(p) = self.pending.get(&id).cloned() {
                        #[cfg(feature = "perf")]
                        if p.kind == Kind::Completion {
                            crate::perf::record("completion.lsp", p.started.elapsed());
                        }
                        if let Some(error) = error {
                            self.pending.remove(&id);
                            // Typing while a request is pending is coalesced. If that
                            // older request fails, dispatch the newer state once rather
                            // than silently losing the input that was coalesced into it.
                            refresh_completion |= p.kind == Kind::Completion
                                && p.revision != docs.editor.buffer.revision
                                && docs.editor.config.completion.enabled
                                && docs.editor.config.completion.auto_trigger
                                && self.completion.as_ref().is_some_and(|c| {
                                    c.id == p.session
                                        && c.mode == docs.editor.mode
                                        && c.compatible(docs.id(), &docs.editor.buffer)
                                });
                            self.message = Some(error);
                            changed = true;
                        } else {
                            self.submit(
                                format!("r:{}:{}:{:?}", p.server, p.document, p.kind),
                                Job::Reply {
                                    pending: p,
                                    value: result,
                                },
                            );
                        }
                    } else {
                        self.retire(result);
                    }
                }
                _ => {}
            }
        }
        let mut ready = Vec::new();
        if let Some(result) = self
            .workspace_task
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.workspace_task = None;
            ready.push(result);
        }
        if let Some(worker) = &mut self.preparation {
            for _ in 0..16 {
                if let Some(r) = worker.poll() {
                    ready.push(r);
                } else {
                    break;
                }
            }
        }
        for result in ready {
            match result {
                Prepared::Workspace(context, result) => {
                    self.workspace_planning = false;
                    match result {
                        Err(error) => {
                            self.workspace_reply(&context, json!({"applied":false,"failureReason":error.to_string(),"failedChange":error.index}));
                            self.message = Some(error.to_string());
                        }
                        Ok(_) if self.workspace_cancel.load(Ordering::Relaxed) => {
                            self.workspace_reply(&context,json!({"applied":false,"failureReason":"Workspace edit cancelled before execution"}));
                            self.message = Some("Workspace edit cancelled before execution".into());
                        }
                        Ok(plan) => {
                            let token = context.token;
                            let needs_confirmation = !plan.confirmations.is_empty();
                            self.workspace_approval = Some(WorkspaceApproval {
                                token,
                                summaries: plan.steps.iter().map(|s| s.summary.clone()).collect(),
                                reasons: plan.confirmations.clone(),
                                plan,
                                context,
                            });
                            if !needs_confirmation {
                                self.workspace_answer(docs, token, true);
                            }
                        }
                    }
                    changed = true;
                }
                Prepared::Committed(context, mut outcome, notifications) => {
                    self.workspace_executing = false;
                    if let Err(error) = reconcile_workspace(docs, &outcome) {
                        if let Some(failure) = &mut outcome.failure {
                            failure
                                .reason
                                .push_str(&format!("; editor reconciliation: {error}"));
                        } else {
                            outcome.failure = Some(error);
                        }
                    }
                    if let Some((key, _, _)) = &context.reply {
                        self.workspace_keep.insert(key.clone());
                    }
                    if let Some((key, _)) = &context.command {
                        self.workspace_keep.insert(key.clone());
                    }
                    self.workspace_invalidate(docs, &outcome, &context.documents);
                    for FileNotifications {
                        key,
                        started,
                        messages,
                    } in notifications
                    {
                        if let Some(server) = self.servers.get(&key)
                            && server.started == started
                        {
                            for (method, params) in messages {
                                server.client.notify(&method, params);
                            }
                        }
                    }
                    self.update(docs);
                    self.workspace_reply(&context, outcome.response());
                    self.workspace_changed = outcome.applied.iter().any(|a| {
                        !matches!(
                            a.step.operation,
                            lsp::workspace::Operation::Text { .. }
                                | lsp::workspace::Operation::Ignore
                        ) || !a.displaced.is_empty()
                    });
                    let message = if let Some(failure) = &outcome.failure {
                        format!(
                            "{failure}; {} changes committed; retained backups: {:?}",
                            outcome.applied.len(),
                            outcome.backups
                        )
                    } else {
                        format!(
                            "Workspace edit applied (text remains unsaved). Retained backups: {:?}",
                            outcome.backups
                        )
                    };
                    self.notification(
                        if outcome.failure.is_some() {
                            super::toast::Category::Error
                        } else {
                            super::toast::Category::Success
                        },
                        "Workspace edit",
                        &message,
                    );
                    self.message = Some(message);
                    if outcome.failure.is_none()
                        && let Some((server, mut command)) = context.command
                    {
                        command["filavimServer"] = Value::String(server);
                        self.request(docs, Kind::Execute, Some(command));
                    }
                    self.workspace_keep.clear();
                    self.update(docs);
                    changed = true;
                }
                Prepared::Diagnostics {
                    epoch,
                    owner,
                    server,
                    document,
                    revision,
                    encoding,
                    items,
                    truncated,
                } => {
                    if epoch == self.diagnostic_epochs.get(&document).copied().unwrap_or(0)
                        && owner.is_none_or(|owner| {
                            self.servers
                                .get(&server)
                                .is_some_and(|s| s.started == owner && !s.stopped)
                        })
                        && docs
                            .get(document)
                            .is_some_and(|e| e.buffer.revision == revision)
                    {
                        if let Some(old) = self
                            .diagnostics
                            .insert((server, document), (revision, encoding, items))
                        {
                            self.retire(old);
                        }
                        if truncated {
                            self.notification(
                                super::toast::Category::Warning,
                                "Diagnostics",
                                "Diagnostics limited to 50,000 entries",
                            );
                        }
                        changed = true;
                    } else {
                        self.retire(items);
                    }
                }
                Prepared::Panel(p, mut rows) => {
                    if p.kind == Kind::Execute {
                        changed = true;
                    }
                    #[cfg(feature = "perf")]
                    if p.kind == Kind::Completion {
                        crate::perf::record("completion.prepared_to_update", p.prepared.elapsed());
                    }
                    if p.kind == Kind::Resolve {
                        if let Some(Row {
                            choice: Choice::Completion(v),
                            ..
                        }) = rows.first_mut()
                        {
                            let source = super::completion::Source {
                                rows: Vec::new(),
                                text: p.snapshot.clone(),
                                revision: p.source_revision,
                                encoding: p.encoding,
                                incomplete: false,
                                query: String::new(),
                            };
                            if super::completion::rebase(v, &source, &docs.editor.buffer).is_err() {
                                self.pending.remove(&p.id);
                                continue;
                            }
                        }
                        if self.pending.remove(&p.id).is_some()
                            && docs.id() == p.document
                            && docs.editor.buffer.revision == p.revision
                            && docs.editor.buffer.cursor == p.cursor
                            && docs.editor.mode == p.mode
                            && let Some(panel) = &mut self.panel
                            && panel.request == p.parent
                            && panel.selected == p.selected
                            && let Some(row) = panel.rows.get_mut(p.selected)
                            && (p.candidate.is_none()
                                || super::completion::identity(row) == p.candidate.as_deref())
                            && let Some(mut resolved) = rows.pop()
                        {
                            if let (Choice::Completion(original), Choice::Completion(update)) =
                                (&row.choice, &mut resolved.choice)
                            {
                                if update["kind"].is_null() {
                                    resolved.label = row.label.clone();
                                }
                                let mut merged = original.clone();
                                if let (Some(base), Some(updates)) =
                                    (merged.as_object_mut(), update.as_object())
                                {
                                    base.extend(
                                        updates
                                            .iter()
                                            .filter(|(k, _)| !k.starts_with("filavim"))
                                            .map(|(k, v)| (k.clone(), v.clone())),
                                    );
                                }
                                merged["filavimResolved"] = Value::Bool(true);
                                *update = merged;
                            }
                            *row = resolved;
                            changed = true;
                            if self.accept_when_ready.take() == Some((p.parent, p.selected)) {
                                self.accept(docs);
                            }
                        } else {
                            self.retire(rows);
                        }
                        continue;
                    }
                    if p.kind == Kind::Completion {
                        if self.pending.remove(&p.id).is_some()
                            && self.completion.as_ref().is_some_and(|c| {
                                c.id == p.session
                                    && c.mode == docs.editor.mode
                                    && c.compatible(docs.id(), &docs.editor.buffer)
                            })
                        {
                            let incomplete = p.incomplete;
                            let old_revision = p.revision;
                            if let Some(session) = &mut self.completion {
                                session.advance(&docs.editor.buffer);
                                let query = p.snapshot.slice(session.anchor..p.cursor).to_string();
                                let old = session.sources.insert(
                                    p.server,
                                    super::completion::Source {
                                        rows,
                                        text: p.snapshot,
                                        revision: p.revision,
                                        encoding: p.encoding,
                                        incomplete,
                                        query,
                                    },
                                );
                                if let Some(old) = old {
                                    self.retire(old);
                                }
                            }
                            self.publish_completion(docs);
                            self.resolve_selected(docs);
                            if old_revision != docs.editor.buffer.revision {
                                self.request_completion(docs, false);
                            }
                            changed = true;
                        } else {
                            self.retire(rows);
                        }
                        continue;
                    }
                    if self.pending.remove(&p.id).is_some()
                        && docs.id() == p.document
                        && docs.editor.buffer.revision == p.revision
                        && docs.editor.buffer.cursor == p.cursor
                        && docs.editor.mode == p.mode
                    {
                        if rows.is_empty() {
                            self.message = Some(format!("No results for {}", p.kind.method()));
                        }
                        let panel = (!rows.is_empty()).then_some(Panel {
                            request: p.id,
                            server: Some(p.server.clone()),
                            title: p.kind.method().trim_start_matches("textDocument/").into(),
                            presentation: match p.kind {
                                Kind::Completion => super::popup::Presentation::Completion,
                                Kind::Hover | Kind::Signature => {
                                    super::popup::Presentation::Information
                                }
                                Kind::Actions => super::popup::Presentation::ContextMenu,
                                _ => super::popup::Presentation::Picker,
                            },
                            rows,
                            selected: 0,
                            document: p.document,
                            revision: p.revision,
                            cursor: p.cursor,
                            encoding: p.encoding,
                            kind: p.kind,
                            mode: p.mode,
                        });
                        self.install_information(p.kind, panel);
                        if p.kind == Kind::Completion {
                            self.resolve_selected(docs);
                        }
                        changed = true;
                    } else {
                        self.retire(rows);
                    }
                }
            }
        }
        if refresh_completion {
            self.request_completion(docs, false);
        }
        changed
    }
    pub fn highlights(
        &self,
        e: &crate::editor::Editor,
        range: std::ops::Range<usize>,
    ) -> Vec<Highlight> {
        let Some((rev, spans)) = self.syntax_spans.get(&e.document_id) else {
            return Vec::new();
        };
        let Some(changes) = e.buffer.changes_since(*rev) else {
            return Vec::new();
        };
        syntax::project_highlights(spans, &changes, range)
    }
    pub fn commit_character(&mut self, docs: &mut Documents, c: char) -> bool {
        if self.needs_resolve() {
            return false;
        }
        let Some(panel) = &self.panel else {
            return false;
        };
        if panel.kind != Kind::Completion || docs.editor.mode != crate::editor::Mode::Insert {
            return false;
        }
        let Some(Row {
            choice: Choice::Completion(value),
            ..
        }) = panel.rows.get(panel.selected)
        else {
            return false;
        };
        if !value["commitCharacters"].as_array().is_some_and(|a| {
            a.iter()
                .any(|v| v.as_str().is_some_and(|s| s.chars().eq(std::iter::once(c))))
        }) {
            return false;
        }
        let revision = docs.editor.buffer.revision;
        self.accept(docs);
        if docs.editor.buffer.revision == revision {
            return false;
        }
        let at = docs.editor.buffer.cursor;
        if matches!(c, '(' | '[' | '{') && at >= 2 && docs.editor.buffer.text.char(at - 2) == c {
            docs.editor.buffer.cursor -= 1;
            docs.editor.adopt_completion_pair();
        } else if at == 0 || docs.editor.buffer.text.char(at - 1) != c {
            docs.editor.type_character(c);
        }
        true
    }
    pub fn pairing_token(&self, docs: &Documents) -> Option<syntax::Token> {
        let e = &docs.editor;
        let (revision, spans) = self.syntax_spans.get(&docs.id())?;
        if *revision != e.buffer.revision {
            return None;
        }
        let at = e.buffer.text.char_to_byte(e.buffer.cursor);
        let end = spans.partition_point(|h| h.range.start < at);
        spans[..end]
            .iter()
            .rev()
            .take(8)
            .find(|h| h.range.start < at && at < h.range.end)
            .map(|h| h.token)
    }
    pub fn typed_trigger(&mut self, docs: &Documents, c: char) {
        let trigger = c.to_string();
        if self.servers.values().any(|s| {
            s.ready
                && s.documents.contains_key(&docs.id())
                && s.capabilities["signatureHelpProvider"]["triggerCharacters"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&trigger)))
        }) {
            self.request(docs,Kind::Signature,Some(json!({"context":{"triggerKind":2,"triggerCharacter":trigger,"isRetrigger":false}})));
        }
    }
    pub fn accept(&mut self, docs: &mut Documents) -> Option<Choice> {
        if self.needs_resolve() {
            let p = self.panel.as_ref()?;
            self.accept_when_ready = Some((p.request, p.selected));
            self.resolve_selected(docs);
            return None;
        }
        self.accept_when_ready = None;
        if let Some(old) = self.completion.take() {
            self.retire(old);
        }
        let panel = self.panel.take()?;
        if panel.document != docs.id()
            || panel.revision != docs.editor.buffer.revision
            || panel.cursor != docs.editor.buffer.cursor
            || panel.mode != docs.editor.mode
        {
            return None;
        }
        let row = panel.rows.get(panel.selected)?.clone();
        let encoding = if let Choice::Completion(v) = &row.choice {
            v["filavimProvider"]
                .as_str()
                .and_then(|k| self.servers.get(k))
                .map_or(panel.encoding, |s| s.encoding)
        } else {
            panel.encoding
        };
        match row.choice {
            Choice::Completion(value) => {
                if let Err(e) = completion(&mut docs.editor, value, encoding) {
                    self.message = Some(e.to_string());
                }
                None
            }
            Choice::Position(position, encoding) => {
                docs.editor
                    .buffer
                    .path
                    .clone()
                    .map(|path| Choice::Location {
                        path,
                        position,
                        encoding,
                    })
            }
            Choice::Action(mut value) => {
                let server = panel.server?;
                if value["edit"].is_null()
                    && value["command"].is_null()
                    && value["filavimResolved"] != true
                {
                    if self.servers.get(&server).is_some_and(|s| {
                        s.capabilities["codeActionProvider"]["resolveProvider"] == true
                    }) {
                        value["filavimServer"] = Value::String(server);
                        self.request(docs, Kind::ActionResolve, Some(value));
                    } else {
                        self.message = Some(
                            "Server returned an action with no supported edit or command".into(),
                        );
                    }
                    return None;
                }
                let command = if value["command"].is_string() {
                    value.clone()
                } else {
                    value["command"].clone()
                };
                let command = command["command"]
                    .is_string()
                    .then_some((server.clone(), command));
                if value["edit"].is_null() {
                    if let Some((server, mut command)) = command {
                        command["filavimServer"] = Value::String(server);
                        self.request(docs, Kind::Execute, Some(command));
                    }
                } else {
                    self.workspace_edit(docs, value["edit"].clone(), panel.encoding, command);
                }
                None
            }
            Choice::Edit(value) => {
                let value = if matches!(panel.kind, Kind::Formatting | Kind::RangeFormatting) {
                    let uri = docs
                        .editor
                        .buffer
                        .path
                        .as_ref()
                        .and_then(|p| lsp::uri(p).ok())
                        .unwrap_or_default();
                    json!({"changes":{uri:value}})
                } else {
                    value
                };
                if value.is_null() {
                    self.message =
                        Some("This action requires a server command; no edit was applied".into());
                    return None;
                }
                self.workspace_edit(docs, value, panel.encoding, None);
                None
            }
            choice => Some(choice),
        }
    }
    pub fn workspace_edit(
        &mut self,
        docs: &Documents,
        value: Value,
        encoding: Encoding,
        command: Option<(String, Value)>,
    ) {
        self.workspace_plan(docs, value, encoding, None, command);
    }
    fn workspace_submit(&mut self, job: WorkspaceJob) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.workspace_task = Some(rx);
        let cancel = self.workspace_cancel.clone();
        let progress = self.workspace_progress.clone();
        std::thread::spawn(move || {
            let result = match job {
                WorkspaceJob::Workspace {
                    value,
                    root,
                    context,
                    encoding,
                } => {
                    let result = lsp::workspace::plan_cancellable(
                        &value,
                        &root,
                        context.documents.clone(),
                        encoding,
                        &cancel,
                    );
                    Prepared::Workspace(context, result)
                }
                WorkspaceJob::CommitWorkspace { plan, context } => {
                    let outcome =
                        lsp::workspace::execute_with_progress(plan, &cancel, |completed, _| {
                            progress.store(completed, Ordering::Relaxed);
                            crate::background::wake();
                        });
                    let notifications = context
                        .subscriptions
                        .iter()
                        .map(|(key, started, options)| FileNotifications {
                            key: key.clone(),
                            started: *started,
                            messages: lsp::workspace::notifications(&outcome, options),
                        })
                        .collect();
                    Prepared::Committed(context, outcome, notifications)
                }
            };
            let _ = tx.send(result);
            crate::background::wake();
        });
    }
    #[cfg(all(test, feature = "gpui-tests"))]
    pub(crate) fn wait_workspace_task(&mut self) {
        if let Some(receiver) = self.workspace_task.take() {
            let result = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("Workspace worker acknowledgement");
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(result).unwrap();
            self.workspace_task = Some(rx);
        }
    }
    pub fn cancel_workspace(&mut self, docs: &Documents) {
        if let Some(approval) = &self.workspace_approval {
            let token = approval.token;
            self.workspace_answer(docs, token, false);
        } else if self.workspace_active() {
            self.workspace_cancel.store(true, Ordering::Relaxed);
            self.message = Some(
                "Workspace cancellation requested; committed changes and backups are retained"
                    .into(),
            );
        }
    }
    pub fn workspace_preparing(&self) -> bool {
        self.workspace_planning
    }
    pub fn workspace_status(&self) -> Option<String> {
        if self.workspace_executing {
            Some(format!(
                "Workspace edit: {}/{} processed · Esc cancels",
                self.workspace_progress.load(Ordering::Relaxed),
                self.workspace_total
            ))
        } else if self.workspace_planning {
            Some("Planning workspace edit · Esc cancels".into())
        } else if self.workspace_approval.is_some() {
            Some("Review workspace edit".into())
        } else {
            None
        }
    }
    pub fn workspace_active(&self) -> bool {
        self.workspace_planning || self.workspace_executing || self.workspace_approval.is_some()
    }
    fn workspace_reply(&mut self, context: &WorkspaceContext, result: Value) {
        if let Some((key, id, started)) = &context.reply {
            self.workspace_requests
                .remove(&format!("{key}:{started:?}:{id}"));
            if let Some(server) = self.servers.get(key)
                && server.started == *started
            {
                server.client.respond(id.clone(), result);
            }
        }
    }
    fn workspace_plan(
        &mut self,
        docs: &Documents,
        value: Value,
        encoding: Encoding,
        reply: Option<(String, Value, std::time::Instant)>,
        command: Option<(String, Value)>,
    ) {
        let documents = docs
            .order
            .iter()
            .filter_map(|id| {
                let e = docs.get(*id)?;
                Some(lsp::workspace::OpenDocument {
                    id: *id,
                    path: e.buffer.path.clone()?,
                    snapshot: lsp::Snapshot {
                        text: e.buffer.text.clone(),
                        revision: e.buffer.revision,
                    },
                    baseline: e.buffer.disk_state.clone(),
                    dirty: e.buffer.dirty(),
                })
            })
            .collect();
        self.serial += 1;
        let context = WorkspaceContext {
            root: self.root.clone(),
            token: self.serial,
            documents,
            reply,
            command,
            subscriptions: Vec::new(),
        };
        if self.workspace_active() || self.file_busy || self.shutting_down {
            self.workspace_reply(&context,json!({"applied":false,"failureReason":"Another file/workspace operation is active; retry after it finishes"}));
            self.message = Some(
                "Workspace edit not started: another file/workspace operation is active".into(),
            );
            return;
        }
        self.workspace_cancel = Default::default();
        self.workspace_progress = Default::default();
        self.workspace_total = 0;
        self.workspace_planning = true;
        self.panel = None;
        self.signature = None;
        self.workspace_submit(WorkspaceJob::Workspace {
            value,
            root: self.root.clone(),
            context,
            encoding,
        });
    }
    pub fn workspace_answer(&mut self, docs: &Documents, token: u64, approved: bool) {
        if self
            .workspace_approval
            .as_ref()
            .is_none_or(|a| a.token != token)
        {
            return;
        }
        let mut approval = self.workspace_approval.take().unwrap();
        let valid = lsp::path_key(&approval.context.root) == lsp::path_key(&self.root)
            && approval.context.documents.iter().all(|d| {
                docs.get(d.id).is_some_and(|e| {
                    e.buffer.path.as_ref() == Some(&d.path)
                        && e.buffer.revision == d.snapshot.revision
                })
            })
            && docs
                .order
                .iter()
                .filter_map(|id| docs.get(*id))
                .filter(|e| e.buffer.path.is_some())
                .count()
                == approval.context.documents.len();
        let owner_alive = approval
            .context
            .reply
            .as_ref()
            .is_none_or(|(key, _, started)| {
                self.servers.get(key).is_some_and(|s| {
                    s.started == *started && !s.stopped && !s.client.stopped.load(Ordering::Acquire)
                })
            });
        if !approved || !valid || !owner_alive || self.file_busy || self.shutting_down {
            let reason = if !approved {
                "Workspace edit rejected/cancelled by user"
            } else {
                "Request owner, document or file-operation state changed; request a fresh workspace edit"
            };
            self.workspace_reply(
                &approval.context,
                json!({"applied":false,"failureReason":reason}),
            );
            self.message = Some(reason.into());
            return;
        }
        approval.context.subscriptions = self
            .servers
            .iter()
            .map(|(key, server)| {
                (
                    key.clone(),
                    server.started,
                    server.capabilities["workspace"]["fileOperations"].clone(),
                )
            })
            .collect();
        self.workspace_total = approval.plan.steps.len();
        self.workspace_executing = true;
        let affected: BTreeSet<_> = approval
            .context
            .documents
            .iter()
            .filter(|d| approval.plan.steps.iter().any(|s| s.touches(&d.path)))
            .map(|d| d.id)
            .collect();
        let cancelled: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.kind != Kind::Execute && affected.contains(&p.document))
            .map(|(id, _)| *id)
            .collect();
        for id in cancelled {
            if let Some(p) = self.pending.remove(&id)
                && let Some(server) = self.servers.get(&p.server)
            {
                server.client.cancel(id);
            }
        }
        self.completion = None;
        self.panel = None;
        self.signature = None;
        self.workspace_submit(WorkspaceJob::CommitWorkspace {
            plan: approval.plan,
            context: approval.context,
        });
    }
    fn workspace_invalidate(
        &mut self,
        docs: &Documents,
        outcome: &lsp::workspace::Outcome,
        before: &[lsp::workspace::OpenDocument],
    ) {
        let mut affected: BTreeSet<_> = before
            .iter()
            .filter(|d| outcome.applied.iter().any(|s| s.touches(&d.path)))
            .map(|d| d.id)
            .collect();
        affected.extend(docs.order.iter().filter_map(|id| {
            docs.get(*id)
                .and_then(|e| e.buffer.path.as_ref())
                .filter(|p| outcome.applied.iter().any(|s| s.touches(p)))
                .map(|_| *id)
        }));
        for id in &affected {
            *self.diagnostic_epochs.entry(*id).or_default() += 1;
            self.syntax_sent.remove(id);
            self.syntax_spans.remove(id);
        }
        self.diagnostics.retain(|(_, id), _| !affected.contains(id));
        for server in self.servers.values_mut() {
            for id in &affected {
                if let Some((uri, _)) = server.documents.remove(id) {
                    server
                        .client
                        .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
                }
            }
        }
    }
    pub fn info(&mut self, docs: &Documents) {
        let rows = self
            .servers
            .iter()
            .map(|(name, s)| Row {
                label: format!(
                    "{}: {} · {} documents · outgoing {} / {} bytes · incoming {}",
                    name,
                    if s.stopped {
                        "stopped"
                    } else if s.ready {
                        "ready"
                    } else {
                        "initializing"
                    },
                    s.documents.len(),
                    s.client.input_metrics.snapshot().depth,
                    s.client.input_metrics.snapshot().approximate_bytes,
                    s.client.event_metrics.snapshot().depth
                ),
                detail: String::new(),
                choice: Choice::Text,
            })
            .collect();
        self.show(docs, "Language servers", rows);
    }
    fn needs_resolve(&self) -> bool {
        self.panel.as_ref().is_some_and(|p| {
            p.kind == Kind::Completion
                && p.rows
                    .get(p.selected)
                    .and_then(|r| {
                        if let Choice::Completion(v) = &r.choice {
                            v["filavimProvider"].as_str()
                        } else {
                            None
                        }
                    })
                    .or(p.server.as_deref())
                    .and_then(|s| self.servers.get(s))
                    .is_some_and(|s| s.resolve)
                && p.rows.get(p.selected).is_some_and(
                    |row| matches!(&row.choice,Choice::Completion(v) if v["filavimResolved"]!=true),
                )
        })
    }
    pub fn selection_changed(&mut self, docs: &Documents) {
        self.accept_when_ready = None;
        self.resolve_selected(docs);
    }
    pub fn resolving_selected(&self) -> bool {
        self.panel.as_ref().is_some_and(|panel| {
            self.pending.values().any(|p| {
                p.kind == Kind::Resolve && p.parent == panel.request && p.selected == panel.selected
            })
        })
    }
    pub fn resolve_selected(&mut self, docs: &Documents) {
        if !self.needs_resolve() {
            return;
        }
        let Some(panel) = &self.panel else {
            return;
        };
        if self.pending.values().any(|p| {
            p.kind == Kind::Resolve && p.parent == panel.request && p.selected == panel.selected
        }) {
            return;
        }
        let provider = panel.rows.get(panel.selected).and_then(|r| {
            if let Choice::Completion(v) = &r.choice {
                v["filavimProvider"].as_str()
            } else {
                None
            }
        });
        let Some(server) = provider.map(str::to_owned).or_else(|| panel.server.clone()) else {
            return;
        };
        let Some(Row {
            choice: Choice::Completion(value),
            ..
        }) = panel.rows.get(panel.selected)
        else {
            return;
        };
        let (parent, selected, encoding) = (
            panel.request,
            panel.selected,
            self.servers[&server].encoding,
        );
        let candidate = super::completion::identity(&panel.rows[panel.selected]).map(str::to_owned);
        let source = self
            .completion
            .as_ref()
            .and_then(|c| c.sources.get(&server));
        let original = source.and_then(|s| {
            s.rows.iter().find_map(|r| {
                if let Choice::Completion(v) = &r.choice {
                    (v["filavimKey"] == value["filavimKey"]).then_some(v)
                } else {
                    None
                }
            })
        });
        let mut value = original.unwrap_or(value).clone();
        let (source_revision, snapshot) = source.map_or_else(
            || (docs.editor.buffer.revision, docs.editor.buffer.text.clone()),
            |s| (s.revision, s.text.clone()),
        );
        if let Some(object) = value.as_object_mut() {
            object.retain(|k, _| !k.starts_with("filavim"));
        }
        let cancelled: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.kind == Kind::Resolve)
            .map(|(id, _)| *id)
            .collect();
        for id in cancelled {
            if let Some(p) = self.pending.remove(&id)
                && let Some(s) = self.servers.get(&p.server)
            {
                s.client.cancel(id);
            }
        }
        self.serial += 1;
        let id = self.serial;
        self.servers[&server]
            .client
            .request(id, Kind::Resolve.method(), value);
        self.pending.insert(
            id,
            Pending {
                id,
                server,
                document: docs.id(),
                revision: docs.editor.buffer.revision,
                cursor: docs.editor.buffer.cursor,
                kind: Kind::Resolve,
                encoding,
                source_revision,
                snapshot,
                session: 0,
                incomplete: false,
                candidate,
                #[cfg(feature = "perf")]
                started: std::time::Instant::now(),
                #[cfg(feature = "perf")]
                prepared: std::time::Instant::now(),
                parent,
                selected,
                mode: docs.editor.mode,
            },
        );
    }
    pub fn dismiss(&mut self) {
        self.signature = None;
        if let Some(old) = self.completion.take() {
            self.retire(old);
        }
        self.accept_when_ready = None;
        self.panel = None;
        let ids: Vec<_> = self.pending.keys().copied().collect();
        for id in ids {
            if let Some(p) = self.pending.remove(&id)
                && let Some(server) = self.servers.get(&p.server)
            {
                server.client.cancel(id);
            }
        }
    }
    pub fn navigate_diagnostic(&mut self, docs: &mut Documents, backwards: bool) {
        let mut positions: Vec<_> = self
            .diagnostics
            .iter()
            .filter(|((_, id), (revision, _, _))| {
                *id == docs.id() && *revision == docs.editor.buffer.revision
            })
            .flat_map(|(_, (_, encoding, items))| {
                items.iter().filter_map(|d| {
                    lsp::offset(&docs.editor.buffer.text, d.range.start, *encoding).ok()
                })
            })
            .collect();
        positions.sort_unstable();
        positions.dedup();
        let cursor = docs.editor.buffer.cursor;
        let position = if backwards {
            positions
                .iter()
                .rev()
                .find(|&&p| p < cursor)
                .or(positions.last())
        } else {
            positions
                .iter()
                .find(|&&p| p > cursor)
                .or(positions.first())
        };
        if let Some(&position) = position {
            docs.editor.buffer.cursor = position;
            docs.editor.buffer.snap_cursor();
            self.diagnostics_at(docs);
        } else {
            self.message = Some("No current diagnostics in this document".into());
        }
    }
    pub fn diagnostics_at(&mut self, docs: &Documents) {
        let row = docs.editor.buffer.row() as u32;
        let mut rows = Vec::new();
        for ((server, id), (revision, _, items)) in &self.diagnostics {
            if *id != docs.id() || *revision != docs.editor.buffer.revision {
                continue;
            }
            for d in items
                .iter()
                .filter(|d| d.range.start.line <= row && d.range.end.line >= row)
                .take(100)
            {
                let severity = match d.severity {
                    1 => "Error",
                    2 => "Warning",
                    3 => "Information",
                    _ => "Hint",
                };
                rows.push(Row {
                    label: format!(
                        "[{severity}] {}\n{} · {}:{}",
                        d.message,
                        server,
                        d.range.start.line + 1,
                        d.range.start.character + 1
                    ),
                    detail: String::new(),
                    choice: Choice::Text,
                });
            }
        }
        if rows.is_empty() {
            rows.push(Row {
                label: "No diagnostics on this line".into(),
                detail: String::new(),
                choice: Choice::Text,
            });
        }
        self.show(docs, "Diagnostics", rows);
        if let Some(p) = &mut self.panel {
            p.presentation = super::popup::Presentation::Information;
        }
    }
    pub fn saved(&mut self, docs: &Documents, document: u64) {
        self.update(docs);
        let Some(editor) = docs.get(document) else {
            return;
        };
        for server in self.servers.values() {
            if let Some(include) = server.save
                && let Some((uri, _)) = server.documents.get(&document)
            {
                if include && editor.buffer.dirty() {
                    continue;
                }
                server
                    .client
                    .saved(uri.clone(), include.then(|| editor.buffer.text.clone()));
            }
        }
    }
    pub fn completion_ready(&self, docs: &Documents) -> bool {
        self.servers
            .values()
            .any(|s| s.ready && !s.stopped && s.documents.contains_key(&docs.id()))
    }
    pub fn show(&mut self, docs: &Documents, title: &str, rows: Vec<Row>) {
        self.signature = None;
        self.serial += 1;
        self.panel = Some(Panel {
            title: title.into(),
            request: self.serial,
            server: None,
            presentation: if rows.iter().all(|r| matches!(r.choice, Choice::Text)) {
                super::popup::Presentation::InformationCentered
            } else {
                super::popup::Presentation::Picker
            },
            rows,
            selected: 0,
            document: docs.id(),
            revision: docs.editor.buffer.revision,
            cursor: docs.editor.buffer.cursor,
            encoding: Encoding::Utf16,
            kind: Kind::Hover,
            mode: docs.editor.mode,
        });
    }
    pub fn show_diagnostics(&mut self, docs: &Documents) {
        let rows = self
            .diagnostics
            .iter()
            .filter(|((_, id), _)| *id == docs.id())
            .flat_map(|((server, _), (revision, encoding, items))| {
                items
                    .iter()
                    .filter(move |_| *revision == docs.editor.buffer.revision)
                    .take(2000)
                    .map(move |d| Row {
                        label: format!(
                            "{}:{} [{}] {}",
                            d.range.start.line + 1,
                            d.range.start.character + 1,
                            server,
                            d.message
                        ),
                        detail: String::new(),
                        choice: docs
                            .editor
                            .buffer
                            .path
                            .clone()
                            .map_or(Choice::Text, |path| Choice::Location {
                                path,
                                position: d.range.start,
                                encoding: *encoding,
                            }),
                    })
            })
            .collect();
        self.show(docs, "Diagnostics", rows);
    }
    fn stop_instance(&mut self, key: &str) {
        if self
            .signature
            .as_ref()
            .is_some_and(|p| p.server.as_deref() == Some(key))
        {
            self.signature = None;
        }
        if let Some(session) = &mut self.completion {
            session.sources.remove(key);
        }
        if let Some(server) = self.servers.remove(key) {
            server.client.shutdown();
            self.retiring
                .insert(key.into(), server.client.stopped.clone());
            self.retired_servers.push(server.client);
            self.pending.retain(|_, p| p.server != key);
            self.diagnostics.retain(|(s, _), _| s != key);
            if self
                .panel
                .as_ref()
                .is_some_and(|p| p.server.as_deref() == Some(key))
            {
                self.panel = None;
            }
        }
    }
    pub fn control(&mut self, id: &str, start: bool, restart: bool) {
        if start {
            self.manual_start = true;
            self.paused.remove(id);
        } else {
            self.paused.insert(id.into());
        }
        let failed = self
            .servers
            .values()
            .any(|s| s.id == id && (s.stopped || s.client.stopped.load(Ordering::Acquire)));
        if !start || restart || failed {
            let keys: Vec<_> = self
                .servers
                .iter()
                .filter(|(_, s)| s.id == id)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys {
                self.stop_instance(&key);
            }
            if !self.shutting_down {
                self.notification(
                    super::toast::Category::Information,
                    "Language server",
                    &format!(
                        "{id}: {}",
                        if start {
                            "restart requested"
                        } else {
                            "stop requested"
                        }
                    ),
                );
            }
        }
    }
    pub fn runtime(&self, id: &str) -> Vec<RuntimeInfo> {
        self.servers
            .values()
            .filter(|s| s.id == id)
            .map(|s| RuntimeInfo {
                root: s.root.clone(),
                command: s.config.command.clone(),
                args: s.config.args.clone(),
                state: if s.failure.is_some() {
                    "Failed"
                } else if s.stopped || s.client.stopped.load(Ordering::Acquire) {
                    "Stopped"
                } else if s.ready {
                    "Running"
                } else {
                    "Starting"
                },
                startup: s.startup,
                failure: s.failure.clone(),
                capabilities: s.capabilities.clone(),
                stderr: s.client.stderr(),
            })
            .collect()
    }
    pub fn paused(&self, id: &str) -> bool {
        self.paused.contains(id)
    }
    pub fn restart(&mut self) {
        self.signature = None;
        if let Some(old) = self.completion.take() {
            self.retire(old);
        }
        for (key, server) in std::mem::take(&mut self.servers) {
            self.retiring.insert(key, server.client.stopped.clone());
            server.client.shutdown();
            self.retired_servers.push(server.client);
        }
        self.pending.clear();
        self.accept_when_ready = None;
        let old = std::mem::take(&mut self.diagnostics);
        if !old.is_empty() {
            self.retire(old);
        }
        self.panel = None;
        if let Some(w) = &mut self.preparation {
            w.cancel_all();
        }
        self.epoch += 1;
    }
    pub fn shutdown(&mut self) {
        if let Some(approval) = self.workspace_approval.take() {
            self.workspace_reply(
                &approval.context,
                json!({"applied":false,"failureReason":"Application shutting down"}),
            );
        }
        self.shutting_down = true;
        self.manager.shutdown();
        for s in self.servers.values() {
            s.client.shutdown();
        }
    }
    pub fn stopped(&self) -> bool {
        !self.workspace_executing
            && !self.workspace_planning
            && !self.manager.busy
            && self
                .servers
                .values()
                .all(|s| s.client.stopped.load(Ordering::Acquire))
            && self
                .retired_servers
                .iter()
                .all(|s| s.stopped.load(Ordering::Acquire))
    }
}
/// Format potentially large Git results on the same background task that executes Git.
pub fn git_rows(event: crate::git::Event) -> (String, Vec<Row>) {
    match event {
        crate::git::Event::Message(message) => (clean(&message, 8192), Vec::new()),
        crate::git::Event::State { state, .. } => {
            let count = state.files.len();
            (
                format!(
                    "Git · {} · {} files{}",
                    state.branch,
                    count,
                    if count > 2000 {
                        " (first 2000 shown)"
                    } else {
                        ""
                    }
                ),
                state
                    .files
                    .into_iter()
                    .take(2000)
                    .map(|(path, status)| Row {
                        label: format!("{} {}", status, clean(&path.to_string_lossy(), 1024)),
                        detail: String::new(),
                        choice: Choice::Location {
                            path,
                            position: lsp_types::Position::new(0, 0),
                            encoding: Encoding::Utf16,
                        },
                    })
                    .collect(),
            )
        }
        crate::git::Event::Diff(state) => (
            "Git diff (bounded preview)".into(),
            state
                .hunks
                .into_iter()
                .take(200)
                .map(|h| Row {
                    label: clean(&h.patch, 8192),
                    detail: String::new(),
                    choice: Choice::Text,
                })
                .collect(),
        ),
        _ => ("Git operation completed".into(), Vec::new()),
    }
}
/// Preflight every planned document against live text/revisions before editing.
/// Applies one undo group per document, opening missing unversioned documents in
/// inactive tabs. This changes buffers only; it does not save files to disk.
pub fn reconcile_workspace(
    docs: &mut Documents,
    outcome: &lsp::workspace::Outcome,
) -> Result<(), lsp::workspace::Failure> {
    use lsp::workspace::Operation;
    let mut grouped = BTreeSet::new();
    let mut failure = None;
    let result = (|| -> Result<(), lsp::workspace::Failure> {
        for applied in &outcome.applied {
            for removed in &applied.displaced {
                for id in docs.order.clone() {
                    let editor = docs.get_mut(id).unwrap();
                    if editor
                        .buffer
                        .path
                        .as_ref()
                        .is_some_and(|p| lsp::workspace::relative_path(p, removed).is_some())
                    {
                        editor.buffer.path = None;
                        editor.buffer.disk_state = None;
                        editor.buffer.mark_recovered();
                        editor.snippet = None;
                    }
                }
            }
            match &applied.step.operation {
                Operation::Text {
                    path,
                    before,
                    edits,
                } => {
                    let id = if let Some(id) = docs.find(path) {
                        id
                    } else {
                        let mut buffer = Buffer::new(before.clone(), Some(path.clone()));
                        buffer.disk_state = applied.baselines.get(path).cloned();
                        docs.add(buffer, false)
                            .map_err(|e| lsp::workspace::Failure {
                                index: applied.step.index,
                                reason: format!("Cannot open workspace document: {e:#}"),
                            })?
                    };
                    let editor = docs.get_mut(id).unwrap();
                    if editor.buffer.text != *before {
                        failure.get_or_insert(lsp::workspace::Failure{index:applied.step.index,reason:"Document changed during workspace execution; disk prefix retained, new editor text preserved".into()});
                        continue;
                    }
                    if grouped.insert(id) {
                        editor.buffer.end_group();
                        editor.buffer.begin_group();
                    }
                    let mut cursor = editor.buffer.cursor;
                    let mut anchor = editor.anchor;
                    for (range, text) in edits {
                        let inserted = text.chars().count();
                        cursor = project_workspace_position(cursor, range, inserted);
                        anchor = anchor.map(|at| project_workspace_position(at, range, inserted));
                        editor.buffer.replace(range.clone(), text);
                    }
                    editor.buffer.cursor = cursor.min(editor.buffer.text.len_chars());
                    editor.buffer.snap_cursor();
                    if let Some(at) = anchor {
                        let saved = editor.buffer.cursor;
                        editor.buffer.cursor = at.min(editor.buffer.text.len_chars());
                        editor.buffer.snap_cursor();
                        editor.anchor = Some(editor.buffer.cursor);
                        editor.buffer.cursor = saved;
                    }
                    editor.snippet = None;
                }
                Operation::Rename { from, to, .. } => {
                    for id in docs.order.clone() {
                        let editor = docs.get_mut(id).unwrap();
                        if let Some(path) = editor
                            .buffer
                            .path
                            .as_ref()
                            .map(|p| PathBuf::from(crate::paths::display_path(p)))
                            && let Some(suffix) = lsp::workspace::relative_path(&path, from)
                        {
                            let path = if suffix.as_os_str().is_empty() {
                                to.clone()
                            } else {
                                to.join(&suffix)
                            };
                            editor.buffer.disk_state = applied.baselines.get(&path).cloned();
                            editor.buffer.path = Some(path);
                            editor.snippet = None;
                        }
                    }
                }
                Operation::Create { path, .. } => {
                    if let Some(id) = docs.find(path) {
                        docs.get_mut(id).unwrap().buffer.disk_state =
                            applied.baselines.get(path).cloned();
                    }
                }
                Operation::Delete { .. } | Operation::Ignore => (),
            }
        }
        if let Some(failure) = failure {
            Err(failure)
        } else {
            Ok(())
        }
    })();
    for id in grouped {
        if let Some(editor) = docs.get_mut(id) {
            editor.buffer.end_group();
        }
    }
    result
}

fn project_workspace_position(at: usize, range: &std::ops::Range<usize>, inserted: usize) -> usize {
    if at >= range.end {
        at.saturating_add_signed(inserted as isize - range.len() as isize)
    } else if at > range.start {
        range.start + inserted
    } else {
        at
    }
}

pub fn apply_plan(docs: &mut Documents, plan: Vec<lsp::PlannedDocument>) -> anyhow::Result<()> {
    for d in &plan {
        if let Some(id) = docs.find(&d.path) {
            let e = docs.get(id).unwrap();
            anyhow::ensure!(
                d.revision == Some(e.buffer.revision) && d.before == e.buffer.text,
                "Document changed before workspace edit"
            );
        } else {
            anyhow::ensure!(
                d.revision.is_none(),
                "Document closed before workspace edit"
            );
        }
    }
    for d in plan {
        let id = if let Some(id) = docs.find(&d.path) {
            id
        } else {
            let mut buffer = Buffer::new(d.before, Some(d.path));
            buffer.disk_state = d.disk_state;
            docs.add(buffer, false)?
        };
        let e = docs.get_mut(id).unwrap();
        e.buffer.end_group();
        e.buffer.begin_group();
        for (r, t) in d.edits {
            e.buffer.replace(r, &t);
        }
        e.buffer.end_group();
    }
    Ok(())
}
fn completion(
    e: &mut crate::editor::Editor,
    value: Value,
    encoding: Encoding,
) -> anyhow::Result<()> {
    let raw = value["textEdit"]["newText"]
        .as_str()
        .or(value["insertText"].as_str())
        .or(value["label"].as_str())
        .unwrap_or("");
    let mut snippet_groups = None;
    let (text, insertion_cursor) = if value["insertTextFormat"].as_u64() == Some(2) {
        let expansion = crate::snippet::normalize(crate::snippet::expand(raw)?, e.buffer.newline);
        let at = expansion
            .fields
            .first()
            .map_or(expansion.text.chars().count(), |r| r.start);
        snippet_groups = (expansion.groups.len() > 1).then_some(expansion.groups);
        (expansion.text, at)
    } else {
        let text = e.buffer.normalize_paste(raw);
        let at = text.chars().count();
        (text, at)
    };
    let mut edits: Vec<lsp_types::TextEdit> = if value["additionalTextEdits"].is_null() {
        Vec::new()
    } else {
        serde_json::from_value(value["additionalTextEdits"].clone())?
    };
    let range = if let Some(range) = value["textEdit"]
        .get("range")
        .or(value["textEdit"].get("replace"))
    {
        serde_json::from_value(range.clone())?
    } else {
        let mut start = e.buffer.cursor;
        while start > 0 {
            let c = e.buffer.text.char(start - 1);
            if super::completion::identifier(c) {
                anyhow::ensure!(
                    e.buffer.cursor - start < 256,
                    "Completion prefix exceeds 256 characters"
                );
                start -= 1;
            } else {
                break;
            }
        }
        lsp::selection_range(&e.buffer.text, start..e.buffer.cursor, encoding)
    };
    let primary_start = lsp::offset(&e.buffer.text, range.start, encoding)?;
    let primary_end = lsp::offset(&e.buffer.text, range.end, encoding)?;
    let primary = e.completion_range(primary_start..primary_end, &text);
    let range = lsp::selection_range(&e.buffer.text, primary, encoding);
    let mut cursor = primary_start + insertion_cursor;
    for edit in &edits {
        let start = lsp::offset(&e.buffer.text, edit.range.start, encoding)?;
        let end = lsp::offset(&e.buffer.text, edit.range.end, encoding)?;
        if end <= primary_start {
            cursor = cursor.saturating_add_signed(
                edit.new_text.chars().count() as isize - (end - start) as isize,
            );
        } else if start < primary_start {
            anyhow::bail!("Additional completion edit overlaps the primary edit");
        }
    }
    edits.push(lsp_types::TextEdit {
        range,
        new_text: text,
    });
    let edits = lsp::ranges(&e.buffer.text, &edits, encoding)?;
    e.buffer.end_group();
    e.buffer.begin_group();
    for (r, t) in edits {
        e.buffer.replace(r, &t);
    }
    e.buffer.end_group();
    e.buffer.cursor = cursor;
    e.buffer.snap_cursor();
    if e.mode == crate::editor::Mode::Insert {
        e.buffer.begin_group();
    }
    e.adopt_completion_pair();
    if let Some(groups) = snippet_groups {
        e.snippet = Some(crate::snippet::Session::new(
            groups,
            cursor.saturating_sub(insertion_cursor),
            &e.buffer,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn typescript(source: &str) -> Documents {
        let mut config = crate::config::Config::default();
        config.lsp.enabled = false;
        config.syntax = false;
        let mut d = Documents::new(super::super::model::Model::new(config).unwrap());
        d.add(
            Buffer::new(ropey::Rope::from_str(source), Some("fixture.ts".into())),
            true,
        )
        .unwrap();
        d.editor.buffer.cursor = d.editor.buffer.text.len_chars();
        d.editor.set_mode(crate::editor::Mode::Insert);
        d
    }
    #[test]
    fn local_types_are_immediate_continuous_and_accept_current_unicode_range() {
        let mut d = typescript("type Config = {\n é😀: ");
        let mut s = Services::new(".".into());
        s.automatic_completion(&d);
        let id = s
            .panel
            .as_ref()
            .expect("keywords after colon/whitespace, without LSP")
            .request;
        for c in "str".chars() {
            d.editor.type_character(c);
            s.update(&d);
            s.automatic_completion(&d);
            let panel = s.panel.as_ref().unwrap();
            assert_eq!(panel.request, id);
            assert!(panel.rows.iter().any(|r| r.label.starts_with("string ")));
        }
        d.editor.buffer.backspace();
        s.update(&d);
        s.automatic_completion(&d);
        assert_eq!(s.panel.as_ref().unwrap().request, id);
        let panel = s.panel.as_mut().unwrap();
        panel.selected = panel
            .rows
            .iter()
            .position(|r| r.label.starts_with("string "))
            .unwrap();
        s.accept(&mut d);
        assert_eq!(
            d.editor.buffer.text.to_string(),
            "type Config = {\n é😀: string"
        );
        assert!(s.panel.is_none());
    }
    #[test]
    fn signature_and_completion_keep_independent_presentation_and_expire_safely() {
        let mut d = typescript("type Config = { name: ");
        let mut s = Services::new(".".into());
        s.automatic_completion(&d);
        let completion = s.panel.as_ref().unwrap().request;
        let mut signature = s.panel.as_ref().unwrap().clone();
        signature.kind = Kind::Signature;
        signature.request = 900;
        signature.presentation = super::super::popup::Presentation::Information;
        s.install_information(Kind::Signature, Some(signature.clone()));
        assert_eq!(s.panel.as_ref().unwrap().request, completion);
        assert_eq!(s.signature.as_ref().unwrap().request, 900);
        s.dismiss();
        assert!(s.panel.is_none() && s.signature.is_none());
        s.install_information(Kind::Signature, Some(signature));
        s.automatic_completion(&d);
        assert_eq!(s.panel.as_ref().unwrap().kind, Kind::Completion);
        assert_eq!(s.signature.as_ref().unwrap().kind, Kind::Signature);
        d.editor.type_character('s');
        s.update(&d);
        assert!(
            s.signature.is_none(),
            "a stale signature must not follow unrelated edits"
        );
        assert!(
            s.panel.is_some(),
            "completion keeps its valid prefix session"
        );
    }
    #[test]
    fn local_selection_survives_replacement_by_equivalent_lsp_item() {
        let d = typescript("type Config = { field: ");
        let mut s = Services::new(".".into());
        s.automatic_completion(&d);
        let panel = s.panel.as_mut().unwrap();
        panel.selected = panel
            .rows
            .iter()
            .position(|r| matches!(&r.choice, Choice::Completion(v) if v["label"] == "number"))
            .unwrap();
        let id = panel.request;
        s.completion.as_mut().unwrap().sources.insert(
            "fixture-lsp".into(),
            super::super::completion::Source {
                rows: vec![completion_row(json!({"label":"number","kind":14}))],
                text: d.editor.buffer.text.clone(),
                revision: d.editor.buffer.revision,
                encoding: Encoding::Utf16,
                incomplete: false,
                query: String::new(),
            },
        );
        s.publish_completion(&d);
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.request, id);
        assert!(
            matches!(&panel.rows[panel.selected].choice, Choice::Completion(v) if v["label"] == "number" && v["filavimProvider"] == "fixture-lsp")
        );
    }
    #[test]
    fn local_types_survive_empty_provider_and_retire_on_rename_or_document_switch() {
        let mut d = typescript("interface User { active: boo");
        let mut s = Services::new(".".into());
        s.automatic_completion(&d);
        let session = s.completion.as_mut().unwrap();
        session.sources.insert(
            "empty-mock".into(),
            super::super::completion::Source {
                rows: vec![],
                text: d.editor.buffer.text.clone(),
                revision: d.editor.buffer.revision,
                encoding: Encoding::Utf16,
                incomplete: false,
                query: "boo".into(),
            },
        );
        s.publish_completion(&d);
        assert!(
            s.panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.label.starts_with("boolean "))
        );
        let source = s
            .completion
            .as_mut()
            .unwrap()
            .sources
            .get_mut("empty-mock")
            .unwrap();
        source.rows = vec![completion_row(json!({"label":"boolean","kind":14}))];
        source.revision = u64::MAX;
        s.publish_completion(&d);
        let rows = &s.panel.as_ref().unwrap().rows;
        assert_eq!(
            rows.len(),
            1,
            "unusable remote ranges must not suppress local types"
        );
        assert!(
            matches!(&rows[0].choice, Choice::Completion(v) if v["filavimProvider"] == super::super::completion::LOCAL_TYPES)
        );
        s.completion
            .as_mut()
            .unwrap()
            .sources
            .get_mut("empty-mock")
            .unwrap()
            .revision = d.editor.buffer.revision;
        s.publish_completion(&d);
        assert_eq!(
            s.panel.as_ref().unwrap().rows.len(),
            1,
            "deduplicate valid LSP/local labels"
        );
        assert!(
            matches!(&s.panel.as_ref().unwrap().rows[0].choice, Choice::Completion(v) if v["filavimProvider"] == "empty-mock")
        );
        d.editor.buffer.path = Some("renamed.js".into());
        s.update(&d);
        assert!(s.panel.is_none() && s.completion.is_none());
        d.add(
            Buffer::new(ropey::Rope::from_str("unrelated"), Some("other.ts".into())),
            true,
        )
        .unwrap();
        s.update(&d);
        assert!(s.panel.is_none());
    }
    #[test]
    #[ignore = "optional installed vtsls; FILAVIM_TEST_VTSLS points to its existing JS entrypoint"]
    fn real_vtsls_type_keywords_and_acceptance() {
        let entry = std::env::var("FILAVIM_TEST_VTSLS")
            .expect("set FILAVIM_TEST_VTSLS to installed vtsls.js");
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"strict":true,"noEmit":true}}"#,
        )
        .unwrap();
        let mut config = crate::config::Config {
            syntax: false,
            ..Default::default()
        };
        config.lsp.servers = vec![lsp::ServerConfig {
            name: "real-vtsls-fixture".into(),
            command: "node".into(),
            args: vec![entry, "--stdio".into()],
            languages: vec!["typescript".into()],
            ..Default::default()
        }];
        let mut d = Documents::new(super::super::model::Model::new(config).unwrap());
        let mut s = Services::new(temp.path().into());
        s.manager =
            crate::lsp_manager::Manager::at(temp.path().join("manager"), temp.path().into());
        for (index, (source, expected)) in [
            ("type Config = { name: str§ }", "string"),
            (
                "interface User { name: string; age: num§; active: boolean }",
                "number",
            ),
            (
                "type Settings = { theme: string; enabled: boo§; retries: number }",
                "boolean",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            s.dismiss();
            let path = temp.path().join(format!("case{index}.ts"));
            let text = source.replace('§', "");
            std::fs::write(&path, &text).unwrap();
            d.add(Buffer::new(ropey::Rope::from_str(&text), Some(path)), true)
                .unwrap();
            d.editor.buffer.cursor = source[..source.find('§').unwrap()].chars().count();
            d.editor.set_mode(crate::editor::Mode::Insert);
            s.automatic_completion(&d);
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !s.completion.as_ref().is_some_and(|c| {
                c.sources
                    .keys()
                    .any(|k| k != super::super::completion::LOCAL_TYPES)
            }) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "vtsls completion timed out: {:?}",
                    s.message
                );
                s.poll(&mut d);
                std::thread::sleep(Duration::from_millis(5));
            }
            let panel = s.panel.as_mut().expect("type keywords must remain visible");
            panel.selected = panel
                .rows
                .iter()
                .position(|r| matches!(&r.choice, Choice::Completion(v) if v["label"] == expected))
                .expect("primitive keyword missing");
            if let Choice::Completion(v) = &panel.rows[panel.selected].choice {
                println!("{expected}: provider {}", v["filavimProvider"]);
            }
            s.accept(&mut d);
            while s.accept_when_ready.is_some() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "vtsls resolve timed out"
                );
                s.poll(&mut d);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                d.editor
                    .buffer
                    .text
                    .to_string()
                    .contains(&format!(": {expected}"))
            );
        }
        s.shutdown();
    }
    #[test]
    fn signature_parameter_colors_use_valid_utf16_boundaries() {
        let signature = |parameter: Value| json!({"signatures":[{"label":"f(😀, value)","parameters":[{"label":parameter}]}]});
        let (text, ranges) = signature_information(&signature(json!([6, 11])));
        assert_eq!(&text[ranges[0].clone()], "value");
        let (text, ranges) = signature_information(&signature(json!("value")));
        assert_eq!(&text[ranges[0].clone()], "value");
        assert!(
            signature_information(&signature(json!([3, 4])))
                .1
                .is_empty()
        );
        assert!(
            signature_information(&signature(json!([6, 999])))
                .1
                .is_empty()
        );
    }
    fn editor(text: &str) -> crate::editor::Editor {
        let mut e = crate::editor::Editor::new(crate::config::Config::default());
        e.buffer = Buffer::new(ropey::Rope::from_str(text), None);
        e
    }
    #[test]
    fn completion_additional_edits_preserve_primary_unicode_cursor_and_undo() {
        let mut e = editor("α\nv");
        e.buffer.cursor = 3;
        let before = e.buffer.text.clone();
        completion(&mut e,json!({"label":"variable","textEdit":{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}},"newText":"😀value"},"additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"use x;\n"}]}),Encoding::Utf16).unwrap();
        assert_eq!(e.buffer.text.to_string(), "use x;\nα\n😀value");
        assert_eq!(e.buffer.cursor, e.buffer.text.len_chars());
        e.buffer.undo();
        assert_eq!(e.buffer.text, before);
    }
    #[test]
    fn diagnostic_uris_follow_platform_path_identity() {
        assert!(same_document_uri(
            "file:///tmp/a%20b.rs",
            "file:///tmp/a%20b.rs"
        ));
        assert!(!same_document_uri("file:///tmp/a.rs", "file:///tmp/b.rs"));
        #[cfg(windows)]
        assert!(same_document_uri(
            "file:///C:/Users/filet/a.rs",
            "file:///c:/Users/filet/a.rs"
        ));
    }
    #[test]
    fn malformed_completion_edits_are_atomic() {
        let mut e = editor("v");
        e.buffer.cursor = 1;
        assert!(
            completion(
                &mut e,
                json!({"label":"value","additionalTextEdits":[{"bad":true}]}),
                Encoding::Utf16
            )
            .is_err()
        );
        assert_eq!(e.buffer.text.to_string(), "v");
        assert_eq!(e.buffer.revision, 0);
    }
}

#[cfg(test)]
mod replay_benchmark {
    use super::*;
    #[test]
    #[ignore = "release-only individual-sample backend/services replay; excludes GPU/display"]
    fn native_services_latency_replay() {
        for loaded in [false, true] {
            let mut config = crate::config::Config::default();
            config.lsp.enabled = false;
            config.syntax = loaded;
            let mut docs = Documents::new(crate::gui::model::Model::new(config).unwrap());
            docs.add(
                Buffer::new(
                    ropey::Rope::from_str(&"fn main() { let value = 42; }\n".repeat(10_000)),
                    Some("replay.rs".into()),
                ),
                true,
            )
            .unwrap();
            let mut services = Services::new(PathBuf::from("."));
            if loaded {
                let diagnostic = json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"synthetic diagnostic","severity":1});
                services.submit(
                    "stress".into(),
                    Job::Diagnostics {
                        epoch: 0,
                        owner: None,
                        server: "fixture".into(),
                        document: docs.id(),
                        revision: 0,
                        encoding: Encoding::Utf16,
                        value: Value::Array(vec![diagnostic; 50_000]),
                    },
                );
            }
            let mut samples = Vec::with_capacity(10_000);
            for i in 0..10_000 {
                let start = std::time::Instant::now();
                docs.editor.buffer.insert("x");
                if i % 64 == 0 {
                    docs.editor.buffer.end_group();
                }
                services.update(&docs);
                services.poll(&mut docs);
                let e = &docs.editor;
                let _fragment = crate::gui::viewport::VisibleLine::new(
                    &e.buffer,
                    0,
                    e.buffer.column(4).saturating_sub(60),
                    80,
                    4,
                );
                samples.push(start.elapsed().as_nanos());
            }
            samples.sort_unstable();
            println!(
                "services_replay loaded={loaded} samples={} p50_ns={} p95_ns={} p99_ns={} max_ns={}",
                samples.len(),
                samples[5000],
                samples[9500],
                samples[9900],
                samples[9999]
            );
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub root: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    pub state: &'static str,
    pub startup: Option<Duration>,
    pub failure: Option<String>,
    pub capabilities: Value,
    pub stderr: String,
}
fn supports(capabilities: &Value, kind: Kind) -> bool {
    let key = match kind {
        Kind::Completion => "completionProvider",
        Kind::Resolve => "completionProvider",
        Kind::Hover => "hoverProvider",
        Kind::Definition => "definitionProvider",
        Kind::Declaration => "declarationProvider",
        Kind::TypeDefinition => "typeDefinitionProvider",
        Kind::References => "referencesProvider",
        Kind::Implementation => "implementationProvider",
        Kind::Signature => "signatureHelpProvider",
        Kind::Formatting => "documentFormattingProvider",
        Kind::RangeFormatting => "documentRangeFormattingProvider",
        Kind::DocumentSymbols => "documentSymbolProvider",
        Kind::WorkspaceSymbols => "workspaceSymbolProvider",
        Kind::Rename => "renameProvider",
        Kind::Actions | Kind::ActionResolve => "codeActionProvider",
        Kind::Execute => "executeCommandProvider",
    };
    capabilities
        .get(key)
        .is_some_and(|v| v == true || v.is_object())
}

#[cfg(test)]
mod completion_pair_tests {
    use super::*;
    use crate::{
        config::Config,
        editor::{Editor, Mode},
        input::Action,
    };
    #[test]
    fn completion_reuses_owned_delimiters_and_snippet_cursor_without_recursive_pairs() {
        let mut e = Editor::new(Config::default());
        e.set_buffer(Buffer::new(
            ropey::Rope::from_str("foo"),
            Some("a.js".into()),
        ));
        e.buffer.cursor = 3;
        e.set_mode(Mode::Insert);
        e.type_character('(');
        completion(&mut e,json!({"label":"foo","textEdit":{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"foo($0)"},"insertTextFormat":2}),Encoding::Utf16).unwrap();
        assert_eq!(e.buffer.text, "foo()");
        assert_eq!(e.buffer.cursor, 4);
        e.type_character(')');
        assert_eq!(e.buffer.text, "foo()");
        assert_eq!(e.buffer.cursor, 5);
        e.act(Action::Undo);
        assert_eq!(
            e.buffer.text, "foo()",
            "completion acceptance is independently undoable"
        );
    }
    #[test]
    fn completion_new_settings_persist_without_touching_user_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.toml");
        let c = crate::setup::save_setting(&path, "pairs.enabled", "false").unwrap();
        assert!(!c.pairs.enabled);
        let c = crate::setup::save_setting(&path, "completion.minimum_prefix", "2").unwrap();
        assert_eq!(c.completion.minimum_prefix, 2);
        let c = crate::setup::save_setting(&path, "completion.matching", "prefix").unwrap();
        assert_eq!(c.completion.matching, "prefix");
        assert!(Config::parse("[pairs.languages]\njs=['abc']").is_err());
        assert!(Config::parse("[completion]\nmatching='invented'").is_err());
    }
}
