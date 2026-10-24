//! Bounded JSON-RPC transport and lazy external server lifecycle, off the UI thread.
use anyhow::{Context, Result, anyhow, bail};
use lsp_types::{Position, Range, TextEdit};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub initialization_options: Value,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            languages: Vec::new(),
            enabled: true,
            initialization_options: Value::Null,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub auto_start: bool,
    pub debounce_ms: u64,
    pub timeout_ms: u64,
    pub servers: Vec<ServerConfig>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_start: true,
            debounce_ms: 120,
            timeout_ms: 10_000,
            servers: [
                ("rust", "rust-analyzer", vec![], vec!["rust"]),
                (
                    "python",
                    "pyright-langserver",
                    vec!["--stdio"],
                    vec!["python"],
                ),
                (
                    "typescript",
                    "typescript-language-server",
                    vec!["--stdio"],
                    vec![
                        "typescript",
                        "javascript",
                        "typescriptreact",
                        "javascriptreact",
                    ],
                ),
                ("clangd", "clangd", vec![], vec!["c", "cpp"]),
                ("go", "gopls", vec![], vec!["go"]),
                (
                    "json",
                    "vscode-json-language-server",
                    vec!["--stdio"],
                    vec!["json", "jsonc"],
                ),
            ]
            .into_iter()
            .map(|(name, cmd, args, langs)| ServerConfig {
                name: name.into(),
                command: cmd.into(),
                args: args.into_iter().map(str::to_owned).collect(),
                languages: langs.into_iter().map(str::to_owned).collect(),
                ..ServerConfig::default()
            })
            .collect(),
        }
    }
}
pub fn language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" => Some("rust"),
        "py" | "pyi" => Some("python"),
        "ts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" | "mjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "hpp" => Some("cpp"),
        "go" => Some("go"),
        "json" => Some("json"),
        "jsonc" => Some("jsonc"),
        _ => None,
    }
}
pub fn uri(path: &Path) -> Result<String> {
    url::Url::from_file_path(Path::new(&crate::workspace::display_path(path)))
        .map(|u| u.to_string())
        .map_err(|_| anyhow!("Not an absolute file path"))
}
pub fn path(uri: &str) -> Result<PathBuf> {
    url::Url::parse(uri)?
        .to_file_path()
        .map_err(|_| anyhow!("Only local file URIs are supported"))
}
pub fn path_key(path: &Path) -> String {
    let s = crate::workspace::display_path(path).replace('\\', "/");
    if cfg!(windows) { s.to_lowercase() } else { s }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}
pub fn position(text: &Rope, index: usize, encoding: Encoding) -> Position {
    let index = index.min(text.len_chars());
    let row = text.char_to_line(index);
    let start = text.line_to_char(row);
    let character = match encoding {
        Encoding::Utf8 => text.char_to_byte(index) - text.char_to_byte(start),
        Encoding::Utf16 => text.char_to_utf16_cu(index) - text.char_to_utf16_cu(start),
        Encoding::Utf32 => index - start,
    };
    Position::new(row as u32, character as u32)
}
pub fn offset(text: &Rope, p: Position, encoding: Encoding) -> Result<usize> {
    let row = p.line as usize;
    if row >= text.len_lines() {
        bail!("LSP line outside document")
    }
    let start = text.line_to_char(row);
    let line = text.line(row);
    let mut end = line.len_chars();
    while end > 0 && matches!(line.char(end - 1), '\r' | '\n') {
        end -= 1;
    }
    let line = line.slice(..end);
    let units = p.character as usize;
    let at = match encoding {
        Encoding::Utf8 if units <= line.len_bytes() => {
            let at = line.byte_to_char(units);
            (line.char_to_byte(at) == units).then_some(at)
        }
        Encoding::Utf16 if units <= line.len_utf16_cu() => {
            let at = line.utf16_cu_to_char(units);
            (line.char_to_utf16_cu(at) == units).then_some(at)
        }
        Encoding::Utf32 if units <= line.len_chars() => Some(units),
        _ => None,
    };
    at.map(|at| start + at)
        .context("LSP position splits a code point or exceeds line")
}

pub fn ranges(
    text: &Rope,
    edits: &[TextEdit],
    encoding: Encoding,
) -> Result<Vec<(std::ops::Range<usize>, String)>> {
    let mut result = edits
        .iter()
        .map(|e| {
            let a = offset(text, e.range.start, encoding)?;
            let b = offset(text, e.range.end, encoding)?;
            if a > b {
                bail!("Reversed text edit")
            };
            Ok((a..b, e.new_text.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    result.sort_by_key(|(r, _)| (r.start, r.end));
    for pair in result.windows(2) {
        if pair[0].0.end > pair[1].0.start || pair[0].0.start == pair[1].0.start {
            bail!("Overlapping/ambiguous LSP edits")
        }
    }
    result.reverse();
    Ok(result)
}
pub fn read_message(reader: &mut impl BufRead) -> Result<Value> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            bail!("Language server closed stdout")
        };
        total += n;
        if total > 8192 {
            bail!("Oversized LSP header")
        };
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length.context("Missing Content-Length")?;
    if length > 16 * 1024 * 1024 {
        bail!("Oversized LSP message")
    };
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    #[cfg(feature = "perf")]
    crate::perf::trace("lsp_response_received", length.to_string());
    let parse = crate::perf::Timer::start();
    let value = serde_json::from_slice(&bytes)?;
    parse.finish("lsp_json_parse");
    Ok(value)
}
pub fn write_message(writer: &mut impl Write, message: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub enum Event {
    Ready {
        capabilities: Value,
        encoding: Encoding,
    },
    Reply {
        id: u64,
        result: Value,
        error: Option<String>,
    },
    Notification {
        method: String,
        params: Value,
    },
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
    Stopped(String),
}
enum Input {
    Send(Value),
    Document(std::sync::Arc<std::sync::Mutex<Option<DocumentUpdate>>>),
    Request(u64, Value),
    Cancel(u64),
    Wire(Result<Value, String>),
    Stop,
}
struct DocumentUpdate {
    uri: String,
    language: String,
    version: u64,
    text: Rope,
    open: bool,
    changes: Option<(u64, Vec<crate::buffer::TextChange>)>,
}
type DocumentSlot = std::sync::Arc<std::sync::Mutex<Option<DocumentUpdate>>>;
type DocumentSlots =
    std::sync::Mutex<BTreeMap<String, std::sync::Weak<std::sync::Mutex<Option<DocumentUpdate>>>>>;
pub struct Client {
    tx: crate::queue::Sender<Input>,
    pub events: crate::queue::Receiver<Event>,
    pub input_metrics: crate::queue::Metrics,
    pub event_metrics: crate::queue::Metrics,
    pub stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    process: SharedProcess,
    documents: DocumentSlots,
}
type SharedProcess = std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>;
struct Events(crate::queue::Sender<Event>);
impl Events {
    fn send(&self, event: Event) -> Result<(), mpsc::SendError<Event>> {
        let bytes = match &event {
            Event::Reply { result, .. } => value_volume(result),
            Event::Notification { params, .. } | Event::ServerRequest { params, .. } => {
                value_volume(params)
            }
            _ => 64,
        };
        let result = self.0.send_sized(event, bytes);
        crate::background::wake();
        result
    }
}
impl Client {
    pub fn start(config: ServerConfig, root: PathBuf, timeout: Duration) -> Self {
        let (tx, rx) = crate::queue::channel();
        let (events_tx, events) = crate::queue::channel();
        let input_metrics = tx.metrics();
        let event_metrics = events_tx.metrics();
        let wire = tx.clone();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = stopped.clone();
        let process = SharedProcess::default();
        let worker_process = process.clone();
        thread::spawn(move || {
            if let Err(e) = serve(
                config,
                root,
                timeout,
                rx,
                wire,
                Events(events_tx.clone()),
                worker_process,
            ) {
                let _ = events_tx.send(Event::Stopped(format!("{e:#}")));
            }
            done.store(true, std::sync::atomic::Ordering::Release);
            crate::background::wake();
        });
        Self {
            tx,
            events,
            input_metrics,
            event_metrics,
            stopped,
            process,
            documents: Default::default(),
        }
    }
    pub fn notify(&self, method: &str, params: Value) {
        self.seal_documents();
        let _ = self.tx.send(Input::Send(
            json!({"jsonrpc":"2.0","method":method,"params":params}),
        ));
    }
    pub fn document(&self, uri: String, language: String, version: u64, text: Rope, open: bool) {
        self.document_changes(uri, language, version, text, open, None);
    }
    pub fn document_changes(
        &self,
        uri: String,
        language: String,
        version: u64,
        text: Rope,
        open: bool,
        changes: Option<(u64, Vec<crate::buffer::TextChange>)>,
    ) {
        let mut slots = self.documents.lock().unwrap();
        if let Some(slot) = slots.get(&uri).and_then(std::sync::Weak::upgrade)
            && let Some(previous) = slot.lock().unwrap().as_mut()
            && !previous.open
            && !open
        {
            if version <= previous.version {
                return;
            }
            previous.changes = match (previous.changes.take(), changes) {
                (Some((base, mut edits)), Some((from, next))) if from == previous.version => {
                    edits.extend(next);
                    if edits.len() <= 256
                        && edits.iter().map(|e| e.text.len()).sum::<usize>() <= 1024 * 1024
                    {
                        Some((base, edits))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            previous.version = version;
            previous.text = text;
            previous.open |= open;
            return;
        }
        slots.retain(|_, slot| slot.strong_count() > 0);
        let slot: DocumentSlot = std::sync::Arc::new(std::sync::Mutex::new(Some(DocumentUpdate {
            uri: uri.clone(),
            language,
            version,
            text,
            open,
            changes,
        })));
        slots.insert(uri, std::sync::Arc::downgrade(&slot));
        let bytes = slot
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |d| d.text.len_bytes());
        let _ = self.tx.send_sized(Input::Document(slot), bytes);
    }
    // Requests/notifications are ordering barriers: edits after one must never be
    // folded into a document update queued before it.
    fn seal_documents(&self) {
        self.documents.lock().unwrap().clear();
    }
    pub fn request(&self, id: u64, method: &str, params: Value) {
        self.seal_documents();
        let bytes = value_volume(&params) + method.len() + 64;
        let _ = self.tx.send_sized(
            Input::Request(
                id,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            ),
            bytes,
        );
    }
    pub fn cancel(&self, id: u64) {
        self.seal_documents();
        let _ = self.tx.send(Input::Cancel(id));
    }
    pub fn respond(&self, id: Value, result: Value) {
        self.seal_documents();
        let _ = self.tx.send(Input::Send(
            json!({"jsonrpc":"2.0","id":id,"result":result}),
        ));
    }
    pub fn shutdown(&self) {
        self.seal_documents();
        let _ = self.tx.send(Input::Stop);
        if !self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            let process = self.process.clone();
            let stopped = self.stopped.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(800));
                if !stopped.load(std::sync::atomic::Ordering::Acquire)
                    && let Some(child) = process.lock().unwrap().as_mut()
                {
                    let _ = child.kill();
                }
            });
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}
struct Process(SharedProcess);
impl Process {
    fn try_wait(&self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.lock().unwrap().as_mut().unwrap().try_wait()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn serve(
    config: ServerConfig,
    root: PathBuf,
    timeout: Duration,
    rx: crate::queue::Receiver<Input>,
    wire: crate::queue::Sender<Input>,
    events: Events,
    process: SharedProcess,
) -> Result<()> {
    let executable = crate::setup::executable(&config.command)
        .with_context(|| format!("{} is missing; open :Setup", config.command))?;
    let mut command = Command::new(executable);
    command
        .args(&config.args)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::setup::hide_window(&mut command);
    let mut spawned = command.spawn()?;
    let stdin = spawned.stdin.take().unwrap();
    let stdout = spawned.stdout.take().unwrap();
    let mut stderr = spawned.stderr.take().unwrap();
    *process.lock().unwrap() = Some(spawned);
    let mut stdin = DeadlineWriter {
        inner: stdin,
        process: process.clone(),
        timeout,
    };
    let child = Process(process);
    thread::spawn(move || {
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    });
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            #[cfg(feature = "perf")]
            crate::perf::trace("lsp_read_started", String::new());
            let result = read_message(&mut reader).map_err(|e| e.to_string());
            #[cfg(feature = "perf")]
            crate::perf::trace(
                "lsp_parsed",
                result
                    .as_ref()
                    .map(|v| v["id"].to_string())
                    .unwrap_or_default(),
            );
            let bytes = result.as_ref().map_or(0, value_volume);
            let stop = result.is_err();
            if wire.send_sized(Input::Wire(result), bytes).is_err() || stop {
                break;
            }
        }
    });
    let root_uri = uri(&root)?;
    write_message(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"processId":std::process::id(),"clientInfo":{"name":"Filavim","version":env!("CARGO_PKG_VERSION")},"rootUri":root_uri,"workspaceFolders":[{"uri":root_uri,"name":root.file_name().unwrap_or_default().to_string_lossy()}],"capabilities":{"general":{"positionEncodings":["utf-16","utf-8","utf-32"]},"workspace":{"applyEdit":true,"workspaceEdit":{"documentChanges":true}},"textDocument":{"completion":{"completionItem":{"snippetSupport":true,"documentationFormat":["markdown","plaintext"],"resolveSupport":{"properties":["detail","documentation","additionalTextEdits"]}}},"signatureHelp":{"contextSupport":true,"signatureInformation":{"documentationFormat":["markdown","plaintext"],"activeParameterSupport":true,"parameterInformation":{"labelOffsetSupport":true}}},"hover":{"contentFormat":["markdown","plaintext"]},"publishDiagnostics":{"versionSupport":true},"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","refactor","source.organizeImports"]}}}}},"initializationOptions":config.initialization_options}}),
    )?;
    let mut initialized = false;
    let mut sync = DocumentSync::default();
    let init_deadline = Instant::now() + timeout;
    let mut queued = Vec::new();
    let mut pending = BTreeMap::new();
    loop {
        let now = Instant::now();
        let wait = if initialized {
            pending
                .values()
                .min()
                .map_or(Duration::from_secs(3600), |deadline: &Instant| {
                    deadline.saturating_duration_since(now)
                })
        } else {
            init_deadline.saturating_duration_since(now)
        };
        let input = match rx.recv_timeout(wait) {
            Ok(i) => Some(i),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => Some(Input::Stop),
        };
        if !initialized && Instant::now() > init_deadline {
            bail!("Language server initialization timed out")
        }
        if !initialized
            && matches!(
                input,
                Some(Input::Send(_) | Input::Document(_) | Input::Request(..) | Input::Cancel(_))
            )
        {
            queued.push(input.unwrap());
            continue;
        }
        match input {
            Some(Input::Stop) => {
                if initialized {
                    write_message(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","id":9_007_199_254_740_991_u64,"method":"shutdown","params":null}),
                    )?;
                    let until = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < until {
                        if let Ok(Input::Wire(Ok(v))) = rx.recv_timeout(Duration::from_millis(20))
                            && v["id"].as_u64() == Some(9_007_199_254_740_991)
                        {
                            break;
                        }
                    }
                    write_message(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}))?;
                    let until = Instant::now() + Duration::from_millis(200);
                    while Instant::now() < until {
                        if child.try_wait()?.is_some() {
                            return Ok(());
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
                return Ok(());
            }
            Some(Input::Send(v)) => sync.send(&mut stdin, &v)?,
            Some(Input::Document(slot)) => {
                let update = { slot.lock().unwrap().take() };
                if let Some(update) = update {
                    write_document(&mut stdin, update, &mut sync)?;
                }
            }
            Some(Input::Request(id, v)) => {
                pending.insert(id, Instant::now() + timeout);
                write_message(&mut stdin, &v)?;
                #[cfg(feature = "perf")]
                crate::perf::trace("lsp_request_written", id.to_string());
            }
            Some(Input::Cancel(id)) => {
                pending.remove(&id);
                write_message(
                    &mut stdin,
                    &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}),
                )?;
            }
            Some(Input::Wire(Err(e))) => bail!("{e}"),
            Some(Input::Wire(Ok(v))) => {
                if let Some(method) = v["method"].as_str() {
                    if let Some(id) = v.get("id") {
                        let result = match method {
                            "workspace/configuration" => Some(Value::Array(vec![
                                    Value::Null;
                                    v["params"]["items"]
                                        .as_array()
                                        .map_or(0, Vec::len)
                                ])),
                            "client/registerCapability"
                            | "client/unregisterCapability"
                            | "window/workDoneProgress/create" => Some(Value::Null),
                            _ => None,
                        };
                        if let Some(result) = result {
                            write_message(
                                &mut stdin,
                                &json!({"jsonrpc":"2.0","id":id,"result":result}),
                            )?;
                        } else {
                            let _ = events.send(Event::ServerRequest {
                                id: id.clone(),
                                method: method.into(),
                                params: v["params"].clone(),
                            });
                        }
                    } else {
                        let _ = events.send(Event::Notification {
                            method: method.into(),
                            params: v["params"].clone(),
                        });
                    }
                } else if v["id"].as_u64() == Some(0) && !initialized {
                    if let Some(error) = v.get("error") {
                        bail!("Initialize failed: {error}")
                    }
                    let capabilities = v["result"]["capabilities"].clone();
                    let encoding = match capabilities["positionEncoding"].as_str() {
                        Some("utf-8") => Encoding::Utf8,
                        Some("utf-32") => Encoding::Utf32,
                        _ => Encoding::Utf16,
                    };
                    sync.encoding = encoding;
                    sync.incremental = capabilities["textDocumentSync"] == 2
                        || capabilities["textDocumentSync"]["change"] == 2;
                    write_message(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
                    )?;
                    initialized = true;
                    let _ = events.send(Event::Ready {
                        capabilities,
                        encoding,
                    });
                    for input in queued.drain(..) {
                        match input {
                            Input::Send(v) => sync.send(&mut stdin, &v)?,
                            Input::Document(slot) => {
                                let update = { slot.lock().unwrap().take() };
                                if let Some(update) = update {
                                    write_document(&mut stdin, update, &mut sync)?;
                                }
                            }
                            Input::Request(id, v) => {
                                pending.insert(id, Instant::now() + timeout);
                                write_message(&mut stdin, &v)?;
                            }
                            Input::Cancel(id) => {
                                pending.remove(&id);
                                write_message(
                                    &mut stdin,
                                    &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}),
                                )?;
                            }
                            _ => {}
                        }
                    }
                } else if let Some(id) = v["id"].as_u64()
                    && pending.remove(&id).is_some()
                {
                    let _ = events.send(Event::Reply {
                        id,
                        result: v["result"].clone(),
                        error: v.get("error").map(ToString::to_string),
                    });
                }
            }
            None => {}
        }
        let expired = pending
            .iter()
            .filter(|(_, d)| **d <= Instant::now())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            pending.remove(&id);
            write_message(
                &mut stdin,
                &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}),
            )?;
            let _ = events.send(Event::Reply {
                id,
                result: Value::Null,
                error: Some("Request timed out".into()),
            });
        }
    }
}
#[derive(Default)]
struct DocumentSync {
    encoding: Encoding,
    incremental: bool,
    documents: BTreeMap<String, (u64, Rope)>,
}
impl DocumentSync {
    fn send(&mut self, writer: &mut impl Write, value: &Value) -> Result<()> {
        if value["method"] == "textDocument/didClose"
            && let Some(uri) = value["params"]["textDocument"]["uri"].as_str()
        {
            self.documents.remove(uri);
        }
        write_message(writer, value)
    }
}
fn write_document(
    writer: &mut impl Write,
    update: DocumentUpdate,
    sync: &mut DocumentSync,
) -> Result<()> {
    let DocumentUpdate {
        uri,
        language,
        version,
        text,
        open,
        changes,
    } = update;
    if !open
        && sync
            .documents
            .get(&uri)
            .is_some_and(|(old, _)| *old >= version)
    {
        return Ok(());
    }
    #[cfg(feature = "perf")]
    crate::perf::trace("lsp_sync_serializing", format!("{uri}@{version}"));
    let (method, params) = if open {
        (
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":language,"version":version,"text":text.to_string()}}),
        )
    } else {
        let incremental = changes.filter(|(base, edits)| {
            sync.incremental
                && sync.documents.get(&uri).is_some_and(|(old, _)| old == base)
                && edits.last().is_some_and(|e| e.revision == version)
        });
        let content = if let Some((_, edits)) = incremental {
            let encoding = match sync.encoding {
                Encoding::Utf8 => 0,
                Encoding::Utf16 => 1,
                Encoding::Utf32 => 2,
            };
            edits
                .into_iter()
                .map(|e| json!({"range":e.ranges[encoding],"text":e.text.as_ref()}))
                .collect::<Vec<_>>()
        } else {
            let mut change = json!({"text":text.to_string()});
            if sync.incremental
                && let Some((_, before)) = sync.documents.get(&uri)
            {
                change["range"] = json!(selection_range(
                    before,
                    0..before.len_chars(),
                    sync.encoding
                ));
            }
            vec![change]
        };
        (
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":version},"contentChanges":content}),
        )
    };
    write_message(
        writer,
        &json!({"jsonrpc":"2.0","method":method,"params":params}),
    )?;
    #[cfg(feature = "perf")]
    crate::perf::trace("lsp_sync_written", format!("{uri}@{version}"));
    sync.documents.insert(uri, (version, text));
    Ok(())
}
#[derive(Clone)]
pub struct Snapshot {
    pub text: Rope,
    pub revision: u64,
}
pub struct PlannedDocument {
    pub path: PathBuf,
    pub before: Rope,
    pub revision: Option<u64>,
    pub edits: Vec<(std::ops::Range<usize>, String)>,
}
pub fn plan_edit(
    edit: &Value,
    snapshots: &BTreeMap<String, Snapshot>,
    encoding: Encoding,
) -> Result<Vec<PlannedDocument>> {
    let mut groups: BTreeMap<String, (Option<u64>, Vec<TextEdit>)> = BTreeMap::new();
    if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            groups.insert(uri.clone(), (None, serde_json::from_value(edits.clone())?));
        }
    }
    if let Some(changes) = edit["documentChanges"].as_array() {
        for change in changes {
            if change.get("kind").is_some() {
                bail!("File create/rename/delete edits are not supported; no edits applied")
            }
            let uri = change["textDocument"]["uri"]
                .as_str()
                .context("Missing document URI")?;
            let version = change["textDocument"]["version"].as_u64();
            if groups.contains_key(uri) {
                bail!("Duplicate document edit")
            };
            groups.insert(
                uri.into(),
                (version, serde_json::from_value(change["edits"].clone())?),
            );
        }
    }
    let mut plan = Vec::new();
    for (uri, (version, edits)) in groups {
        let path = path(&uri)?;
        let snapshot = snapshots.get(&path_key(&path));
        if let (Some(expected), Some(snapshot)) = (version, snapshot)
            && expected != snapshot.revision
        {
            bail!("Stale document version")
        }
        if version.is_some() && snapshot.is_none() {
            bail!("Versioned edit for unopened document")
        }
        let before = match snapshot {
            Some(s) => s.text.clone(),
            None => Rope::from_str(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("Cannot load {}", path.display()))?,
            ),
        };
        let edits = ranges(&before, &edits, encoding)?;
        plan.push(PlannedDocument {
            path,
            before,
            revision: snapshot.map(|s| s.revision),
            edits,
        });
    }
    Ok(plan)
}
pub fn selection_range(text: &Rope, range: std::ops::Range<usize>, encoding: Encoding) -> Range {
    Range::new(
        position(text, range.start, encoding),
        position(text, range.end, encoding),
    )
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use crate::buffer::Buffer;
    fn update(buffer: &Buffer, base: u64) -> DocumentUpdate {
        DocumentUpdate {
            uri: "file:///test.rs".into(),
            language: "rust".into(),
            version: buffer.revision,
            text: buffer.text.clone(),
            open: false,
            changes: buffer.changes_since(base).map(|e| (base, e)),
        }
    }
    #[test]
    fn incremental_changes_roundtrip_unicode_multiline_and_undo() {
        for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            let mut b = Buffer::new(Rope::from_str("a😀é\r\nnext"), None);
            let original = b.text.clone();
            b.replace(2..3, "日本\n");
            b.replace(1..2, "🦀");
            let mut sync = DocumentSync {
                incremental: true,
                encoding,
                documents: BTreeMap::from([("file:///test.rs".into(), (0, original.clone()))]),
            };
            let mut wire = Vec::new();
            write_document(&mut wire, update(&b, 0), &mut sync).unwrap();
            let message = read_message(&mut std::io::Cursor::new(wire)).unwrap();
            let changes = message["params"]["contentChanges"].as_array().unwrap();
            assert_eq!(changes.len(), 2);
            let mut replay = original;
            for change in changes {
                let range: lsp_types::Range =
                    serde_json::from_value(change["range"].clone()).unwrap();
                let a = offset(&replay, range.start, encoding).unwrap();
                let z = offset(&replay, range.end, encoding).unwrap();
                replay.remove(a..z);
                replay.insert(a, change["text"].as_str().unwrap());
            }
            assert_eq!(replay, b.text);
            let revision = b.revision;
            assert!(b.undo());
            assert!(b.changes_since(revision).is_none());
            let mut wire = Vec::new();
            write_document(&mut wire, update(&b, revision), &mut sync).unwrap();
            let message = read_message(&mut std::io::Cursor::new(wire)).unwrap();
            assert_eq!(
                message["params"]["contentChanges"][0]["text"],
                b.text.to_string()
            );
        }
    }
    #[test]
    fn coalescing_preserves_request_barriers() {
        let (tx, rx) = crate::queue::channel();
        let (_, events) = crate::queue::channel();
        let client = Client {
            input_metrics: tx.metrics(),
            event_metrics: Default::default(),
            tx,
            events,
            stopped: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            process: Default::default(),
            documents: Default::default(),
        };
        let mut b = Buffer::default();
        b.insert("a");
        client.document_changes(
            "file:///test.rs".into(),
            "rust".into(),
            1,
            b.text.clone(),
            false,
            Some((0, b.changes_since(0).unwrap())),
        );
        b.insert("b");
        client.document_changes(
            "file:///test.rs".into(),
            "rust".into(),
            2,
            b.text.clone(),
            false,
            Some((1, b.changes_since(1).unwrap())),
        );
        client.request(9, "completion", json!({}));
        b.insert("c");
        client.document_changes(
            "file:///test.rs".into(),
            "rust".into(),
            3,
            b.text.clone(),
            false,
            Some((2, b.changes_since(2).unwrap())),
        );
        let Input::Document(slot) = rx.try_recv().unwrap() else {
            panic!()
        };
        let first = slot.lock().unwrap().take().unwrap();
        assert_eq!(first.version, 2);
        assert_eq!(first.text, "ab");
        assert_eq!(first.changes.unwrap().1.len(), 2);
        assert!(matches!(rx.try_recv(), Ok(Input::Request(9, _))));
        let Input::Document(slot) = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(slot.lock().unwrap().take().unwrap().text, "abc");
    }
    #[test]
    #[ignore = "release microbenchmark: cargo test --release sync_payload_benchmark -- --ignored --nocapture"]
    fn sync_payload_benchmark() {
        let mut buffer = Buffer::new(Rope::from_str(&"x".repeat(1_000_000)), None);
        let before = buffer.text.clone();
        buffer.cursor = 500_000;
        buffer.insert("é");
        for incremental in [false, true] {
            let at = Instant::now();
            let mut bytes = 0;
            for _ in 0..100 {
                let mut sync = DocumentSync {
                    incremental,
                    encoding: Encoding::Utf16,
                    documents: BTreeMap::from([("file:///test.rs".into(), (0, before.clone()))]),
                };
                let mut wire = Vec::new();
                write_document(&mut wire, update(&buffer, 0), &mut sync).unwrap();
                bytes = wire.len();
            }
            println!(
                "sync incremental={incremental} bytes={bytes} us/op={:.2}",
                at.elapsed().as_secs_f64() * 1e4
            );
            if incremental {
                assert!(bytes < 400);
            } else {
                assert!(bytes > 1_000_000);
            }
        }
    }
}

fn value_volume(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(value_volume).sum::<usize>() + items.len() * 8,
        Value::Object(items) => items
            .iter()
            .map(|(key, value)| key.len() + value_volume(value) + 16)
            .sum(),
        _ => 8,
    }
}

// One lazy watchdog shared by all language servers. It sleeps on a condition
// variable when no write is active; there is no idle polling or UI-thread wait.
type WriteRegistry = std::sync::Arc<(std::sync::Mutex<WriteState>, std::sync::Condvar)>;
type WeakProcess = std::sync::Weak<std::sync::Mutex<Option<std::process::Child>>>;
#[derive(Default)]
struct WriteState {
    serial: u64,
    active: BTreeMap<u64, (Instant, WeakProcess)>,
}
fn write_registry() -> &'static WriteRegistry {
    static REGISTRY: std::sync::OnceLock<WriteRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = WriteRegistry::default();
        let worker = registry.clone();
        thread::spawn(move || {
            loop {
                let expired = {
                    let mut state = worker.0.lock().unwrap();
                    loop {
                        let next = state
                            .active
                            .iter()
                            .min_by_key(|(_, (deadline, _))| *deadline)
                            .map(|(id, (at, _))| (*id, *at));
                        match next {
                            None => state = worker.1.wait(state).unwrap(),
                            Some((id, at)) => {
                                if let Some(wait) = at.checked_duration_since(Instant::now()) {
                                    state = worker.1.wait_timeout(state, wait).unwrap().0;
                                } else {
                                    break state.active.remove(&id).unwrap().1;
                                }
                            }
                        }
                    }
                };
                if let Some(process) = expired.upgrade()
                    && let Some(child) = process.lock().unwrap().as_mut()
                {
                    let _ = child.kill();
                }
            }
        });
        registry
    })
}
struct WriteLease(u64);
impl Drop for WriteLease {
    fn drop(&mut self) {
        let r = write_registry();
        r.0.lock().unwrap().active.remove(&self.0);
        r.1.notify_one();
    }
}
struct DeadlineWriter {
    inner: std::process::ChildStdin,
    process: SharedProcess,
    timeout: Duration,
}
impl DeadlineWriter {
    fn lease(&self) -> WriteLease {
        let r = write_registry();
        let mut state = r.0.lock().unwrap();
        state.serial += 1;
        let id = state.serial;
        state.active.insert(
            id,
            (
                Instant::now() + self.timeout,
                std::sync::Arc::downgrade(&self.process),
            ),
        );
        r.1.notify_one();
        WriteLease(id)
    }
}
impl Write for DeadlineWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _lease = self.lease();
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let _lease = self.lease();
        self.inner.flush()
    }
}
