//! Isolated, receipt-owned tools. Disk/network/process work runs only on job threads.
//!
//! [`crate::lsp_catalog`] supplies trusted package definitions; this module owns
//! install receipts, preferences, executable resolution and operation progress.
//! Running workspace clients belong to [`crate::gui::services`], not the installer.
//! Uninstallation must remain restricted to installations owned by valid receipts.
#[path = "lsp_install.rs"]
mod managed;
use crate::{
    lsp::ServerConfig,
    lsp_catalog::{self, Entry, Method},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    pub command: PathBuf,
    pub args: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub disabled: BTreeSet<String>,
    pub enabled: BTreeSet<String>,
    pub primary: BTreeMap<String, String>,
    pub custom: BTreeMap<String, Launch>,
    pub settings: BTreeMap<String, Value>,
    pub dismissed: BTreeSet<String>,
    pub all_disabled: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub directory: PathBuf,
    pub version: String,
    pub launch: Launch,
    pub source: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedVersion {
    pub version: String,
    pub checked_unix: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub latest: BTreeMap<String, CachedVersion>,
    pub installed: BTreeMap<String, Installation>,
    pub previous: BTreeMap<String, Installation>,
    pub global: Preferences,
    pub workspaces: BTreeMap<String, Preferences>,
    pub trusted_workspaces: BTreeMap<String, [u64; 2]>,
}
impl Store {
    pub fn preferences(&self, workspace: &Path) -> Preferences {
        let mut p = self.global.clone();
        if let Some(local) = self.workspaces.get(&crate::lsp::path_key(workspace)) {
            for id in &local.enabled {
                p.disabled.remove(id);
            }
            p.enabled.extend(local.enabled.clone());
            p.disabled.extend(local.disabled.clone());
            p.primary.extend(local.primary.clone());
            p.custom.extend(local.custom.clone());
            p.settings.extend(local.settings.clone());
            p.dismissed.extend(local.dismissed.clone());
            if local.all_disabled.is_some() {
                p.all_disabled = local.all_disabled;
            }
        }
        p
    }
}
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub launch: Option<Launch>,
    pub version: Option<String>,
    pub managed: bool,
    pub available: Option<String>,
    pub warning: Option<String>,
    pub dependency_warning: Option<String>,
    pub origin: Option<LaunchOrigin>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchOrigin {
    Managed,
    Configured,
    Workspace,
    System,
}
impl Status {
    pub fn update_available(&self) -> bool {
        self.managed
            && self
                .available
                .as_ref()
                .zip(self.version.as_ref())
                .is_some_and(|(a, b)| a.trim_start_matches('v') != b.trim_start_matches('v'))
    }
}
#[derive(Clone, Debug)]
pub struct Resolved {
    pub id: String,
    pub root: PathBuf,
    pub config: ServerConfig,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub store: Store,
    pub statuses: BTreeMap<String, Status>,
    pub resolved: BTreeMap<String, Vec<Resolved>>,
    pub missing: BTreeSet<String>,
    pub trusted: bool,
    pub trusted_paths: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub enum Change {
    Enable(String, bool),
    Select(String),
    Custom(String, Launch),
    Settings(String, Value),
    Dismiss(Vec<String>),
    All(bool),
}
#[derive(Clone, Debug)]
pub enum Operation {
    Refresh,
    CheckUpdates,
    Install(String),
    Rollback(String),
    Remove(String),
    RepairRuntime,
    Cleanup,
    TrustWorkspace(bool),
    Configure { workspace: bool, change: Change },
}
enum Event {
    Progress(String),
    Complete(PathBuf, Box<Result<Snapshot, String>>),
}
pub struct Manager {
    pub data: PathBuf,
    pub snapshot: Snapshot,
    pub loaded: bool,
    pub has_snapshot: bool,
    pub busy: bool,
    pub progress: String,
    pub operation: Option<Operation>,
    pub error: Option<String>,
    workspace: PathBuf,
    paths: BTreeSet<PathBuf>,
    refresh_needed: bool,
    closed: bool,
    tx: mpsc::SyncSender<Event>,
    rx: mpsc::Receiver<Event>,
    cancel: Arc<AtomicBool>,
}
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("FILAVIM_LSP_HOME") {
        return p.into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home.join("AppData/Local"))
            .join("filavim/lsp")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/filavim/lsp")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".local/share"))
            .join("filavim/lsp")
    }
}
impl Manager {
    pub fn new(workspace: PathBuf) -> Self {
        Self::at(data_dir(), workspace)
    }
    pub fn at(data: PathBuf, workspace: PathBuf) -> Self {
        let (tx, rx) = mpsc::sync_channel(16);
        Self {
            data,
            workspace,
            snapshot: Snapshot::default(),
            paths: BTreeSet::new(),
            loaded: false,
            has_snapshot: false,
            busy: false,
            progress: String::new(),
            operation: None,
            error: None,
            refresh_needed: false,
            closed: false,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn set_workspace(&mut self, workspace: &Path) {
        if self.workspace != workspace {
            self.workspace = workspace.into();
            self.loaded = false;
            self.has_snapshot = false;
            self.snapshot.resolved.clear();
            self.snapshot.missing.clear();
            self.snapshot.trusted = false;
            self.snapshot.trusted_paths.clear();
            self.refresh_needed = true;
        }
    }
    pub fn observe(&mut self, paths: BTreeSet<PathBuf>) {
        if self.closed {
            return;
        }
        if paths != self.paths {
            self.paths = paths;
            self.refresh_needed = true;
        }
        if (!self.loaded || self.refresh_needed) && !self.busy {
            self.submit(Operation::Refresh);
        }
    }
    pub fn submit(&mut self, operation: Operation) -> bool {
        if self.busy || self.closed {
            return false;
        }
        self.busy = true;
        self.error = None;
        self.refresh_needed = false;
        self.operation = Some(operation.clone());
        self.progress = match &operation {
            Operation::Install(id) => format!("Installing / updating {id}"),
            Operation::Rollback(id) => format!("Validating previous {id} installation"),
            Operation::Remove(id) => format!("Removing managed {id}"),
            _ => "Resolving tools and settings".into(),
        };
        self.cancel = Arc::new(AtomicBool::new(false));
        let (data, workspace, paths, tx, cancel) = (
            self.data.clone(),
            self.workspace.clone(),
            self.paths.clone(),
            self.tx.clone(),
            self.cancel.clone(),
        );
        std::thread::spawn(move || {
            let report = |s: &str| {
                let _ = tx.try_send(Event::Progress(s.into()));
                crate::background::wake();
            };
            let result = execute(&data, &workspace, &paths, operation, &cancel, &report)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Complete(workspace, Box::new(result)));
            crate::background::wake();
        });
        true
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn shutdown(&mut self) {
        self.closed = true;
        self.refresh_needed = false;
        self.cancel();
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            changed = true;
            match event {
                Event::Progress(s) => self.progress = s,
                Event::Complete(workspace, result) => {
                    self.busy = false;
                    self.loaded = true;
                    self.progress.clear();
                    if workspace != self.workspace {
                        self.refresh_needed = true;
                        continue;
                    }
                    match *result {
                        Ok(s) => {
                            self.snapshot = s;
                            self.has_snapshot = true;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
            }
        }
        if self.refresh_needed && !self.busy && !self.closed {
            self.submit(Operation::Refresh);
        }
        changed
    }
    pub fn preferences(&self) -> Preferences {
        self.snapshot.store.preferences(&self.workspace)
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn check(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Operation cancelled; previous installation retained"
    );
    Ok(())
}
fn load(data: &Path) -> Result<Store> {
    match std::fs::read(data.join("state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .context("Invalid LSP state.json; repair or move it before managing tools"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e.into()),
    }
}
fn save(data: &Path, store: &Store) -> Result<()> {
    std::fs::create_dir_all(data)?;
    let mut file = tempfile::NamedTempFile::new_in(data)?;
    file.write_all(&serde_json::to_vec_pretty(store)?)?;
    file.as_file().sync_all()?;
    file.persist(data.join("state.json"))
        .map_err(|e| e.error)
        .context("Cannot atomically save LSP preferences")?;
    Ok(())
}
fn tool(name: &str) -> Result<PathBuf> {
    crate::setup::executable(name)
        .filter(|path| path.is_absolute())
        .with_context(|| {
            format!("Missing dependency: {name}. Install it and reopen the manager / Refresh.")
        })
}
fn command(program: &Path, args: &[String]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    crate::setup::hide_window(&mut c);
    c
}
/// No shell strings. Log files avoid pipe deadlocks and bound returned UI text.
fn run(cmd: Command, cancel: &AtomicBool, seconds: u64) -> Result<String> {
    run_with_tick(cmd, cancel, seconds, || {})
}
fn run_with_tick(
    mut cmd: Command,
    cancel: &AtomicBool,
    seconds: u64,
    mut tick: impl FnMut(),
) -> Result<String> {
    check(cancel)?;
    let out = tempfile::tempfile()?;
    let err = tempfile::tempfile()?;
    crate::setup::hide_window(&mut cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(out.try_clone()?))
        .stderr(Stdio::from(err.try_clone()?))
        .spawn()
        .context("Cannot start tool")?;
    let started = Instant::now();
    let mut last_tick = Instant::now();
    let status = loop {
        if last_tick.elapsed() >= Duration::from_millis(250) {
            tick();
            last_tick = Instant::now();
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Acquire)
            || started.elapsed() > Duration::from_secs(seconds)
            || out.metadata()?.len() > 8 * 1024 * 1024
            || err.metadata()?.len() > 8 * 1024 * 1024
        {
            #[cfg(windows)]
            {
                let mut kill = Command::new("taskkill.exe");
                kill.args(["/PID", &child.id().to_string(), "/T", "/F"]);
                crate::setup::hide_window(&mut kill);
                let _ = kill.stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("Operation cancelled or timed out; active installation unchanged");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    use std::io::{Seek, SeekFrom};
    let read = |mut f: std::fs::File| -> Result<String> {
        f.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        f.take(32768).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let (out, err) = (read(out)?, read(err)?);
    ensure!(status.success(), "Tool exited {status}: {out}\n{err}");
    Ok(if out.trim().is_empty() { err } else { out })
}
fn npm() -> Result<(PathBuf, PathBuf)> {
    let node = tool("node")?;
    // Invoke npm's JavaScript entry directly; Windows .cmd shims require a shell.
    let shim = tool("npm")?;
    let parent = shim.parent().context("npm has no parent directory")?;
    let candidates = [
        parent.join("node_modules/npm/bin/npm-cli.js"),
        parent.join("../lib/node_modules/npm/bin/npm-cli.js"),
        parent.join("../share/nodejs/npm/bin/npm-cli.js"),
    ];
    let cli = candidates
        .into_iter()
        .find(|p| p.is_file())
        .context("Cannot resolve npm-cli.js beside npm; reinstall Node.js/npm")?;
    Ok((node, cli))
}
fn python() -> Result<(PathBuf, Vec<String>)> {
    let names = if cfg!(windows) {
        ["py", "python", "python3"]
    } else {
        ["python3", "python", "py"]
    };
    for name in names {
        if let Some(p) = crate::setup::executable(name) {
            if cfg!(windows)
                && p.to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("\\windowsapps\\")
            {
                continue;
            }
            let args = if name == "py" {
                vec!["-3".into()]
            } else {
                vec![]
            };
            let mut c = command(&p, &args);
            c.arg("--version");
            if run(c, &AtomicBool::new(false), 3).is_ok_and(|s| s.starts_with("Python 3.")) {
                return Ok((p, args));
            }
        }
    }
    bail!(
        "Python 3 with venv/pip is required; Windows App Execution Alias is not a Python installation"
    )
}
fn json_url(url: &str, cancel: &AtomicBool) -> Result<Value> {
    ensure!(url.starts_with("https://"), "HTTPS source required");
    let temp = tempfile::NamedTempFile::new()?;
    download(url, temp.path(), cancel, 4 * 1024 * 1024)?;
    serde_json::from_reader(temp.reopen()?).context("Package source returned invalid JSON")
}
fn download(url: &str, dest: &Path, cancel: &AtomicBool, max: u64) -> Result<()> {
    download_progress(url, dest, cancel, max, &|_| {})
}
enum DownloadSource {
    Trusted,
    #[cfg(test)]
    Loopback(String),
}
impl DownloadSource {
    fn validate(&self, address: &str) -> Result<()> {
        match self {
            Self::Trusted => managed::trusted(address),
            #[cfg(test)]
            Self::Loopback(origin) => {
                let url = url::Url::parse(address)?;
                ensure!(
                    url.origin().ascii_serialization() == *origin
                        && url.host_str() == Some("127.0.0.1"),
                    "Fixture redirect escaped loopback origin"
                );
                Ok(())
            }
        }
    }
    fn protocol(&self) -> &'static str {
        match self {
            Self::Trusted => "=https",
            #[cfg(test)]
            Self::Loopback(_) => "=http",
        }
    }
}
fn download_progress(
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    max: u64,
    report: &impl Fn(&str),
) -> Result<()> {
    download_from(url, dest, cancel, max, report, DownloadSource::Trusted)
}
fn download_from(
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    max: u64,
    report: &impl Fn(&str),
    source: DownloadSource,
) -> Result<()> {
    let mut url = url.to_owned();
    for _ in 0..6 {
        source.validate(&url)?;
        let headers = tempfile::NamedTempFile::new()?;
        let mut c = Command::new(tool("curl")?);
        c.args([
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            source.protocol(),
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "--retry",
            "2",
            "--max-filesize",
            &max.to_string(),
            "--user-agent",
            "Filavim-LSP-Manager",
            "--dump-header",
        ])
        .arg(headers.path())
        .args(["--write-out", "%{http_code}", "--output"])
        .arg(dest)
        .arg(&url);
        let status = run_with_tick(c, cancel, 190, || {
            if let Ok(meta) = std::fs::metadata(dest) {
                let total = std::fs::read_to_string(headers.path())
                    .ok()
                    .filter(|s| s.len() <= 65536)
                    .and_then(|text| {
                        text.lines()
                            .filter_map(|line| line.split_once(':'))
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
                            .next_back()
                    });
                if let Some(total) = total {
                    report(&format!("Downloading: {} / {total} bytes", meta.len()));
                } else {
                    report(&format!("Downloading: {} bytes received", meta.len()));
                }
            }
        })?;
        ensure!(
            headers.as_file().metadata()?.len() <= 65536,
            "Response headers exceed limit"
        );
        if status.trim().starts_with('3') {
            let text = std::fs::read_to_string(headers.path())?;
            let location = text
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.trim())
                .context("Redirect has no location")?;
            url = url::Url::parse(&url)?.join(location)?.to_string();
            continue;
        }
        ensure!(
            status.trim() == "200",
            "Unexpected download response {status}"
        );
        ensure!(
            std::fs::metadata(dest)?.len() <= max,
            "Download exceeds size limit"
        );
        return Ok(());
    }
    bail!("Too many download redirects")
}

fn package_url(entry: &Entry) -> String {
    entry.update_source.into()
}

fn available(entry: &Entry, cancel: &AtomicBool) -> Result<(String, Value)> {
    let mut value = json_url(&package_url(entry), cancel)?;
    if let Method::Python(package) = entry.method {
        let tag = value["tag_name"]
            .as_str()
            .context("Release source has no stable tag")?
            .trim_start_matches('v');
        ensure!(
            !tag.is_empty()
                && tag.len() < 100
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c)),
            "Invalid Python release tag"
        );
        value = json_url(
            &format!("https://pypi.org/pypi/{package}/{tag}/json"),
            cancel,
        )?;
    }
    let version = match entry.method {
        Method::Npm(_) => value["version"].as_str(),
        Method::Python(_) => value["info"]["version"].as_str(),
        Method::GithubRust | Method::GithubClang => value["tag_name"].as_str(),
    }
    .context("Source did not report a stable version")?
    .to_owned();
    ensure!(
        !version.is_empty()
            && version.len() < 100
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c)),
        "Invalid package version"
    );
    ensure!(
        value["prerelease"] != true && value["draft"] != true,
        "Stable release required"
    );
    if matches!(entry.method, Method::Npm(_)) {
        ensure!(
            !version.contains('-'),
            "Prerelease npm packages are not installed by default"
        );
    }
    if entry.id == "typescript-native" {
        ensure!(
            version
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .is_some_and(|n| n >= 7),
            "Stable TypeScript source does not provide version 7 or newer"
        );
    }
    Ok((version, value))
}
fn npm_launch(prefix: &Path, package: &str, bin: &str) -> Result<Launch> {
    let directory = prefix.join("node_modules").join(package);
    let node = std::fs::read(prefix.join(".filavim-node.json"))
        .ok()
        .map(|bytes| serde_json::from_slice::<PathBuf>(&bytes))
        .transpose()?;
    package_launch_with_node(&directory, bin, node)
}
fn package_launch(directory: &Path, bin: &str) -> Result<Launch> {
    package_launch_with_node(directory, bin, None)
}
fn package_launch_with_node(directory: &Path, bin: &str, node: Option<PathBuf>) -> Result<Launch> {
    let value: Value = serde_json::from_slice(&std::fs::read(directory.join("package.json"))?)?;
    let relative = value["bin"]
        .as_str()
        .or_else(|| value["bin"][bin].as_str())
        .context("Package does not expose expected executable")?;
    let script = directory.join(relative).canonicalize()?;
    ensure!(
        script.starts_with(directory.canonicalize()?),
        "Package executable escapes its package directory"
    );
    Ok(Launch {
        command: match node {
            Some(node) => node,
            None => tool("node")?,
        },
        args: vec![script.to_string_lossy().into_owned()],
    })
}
fn validate_launch(launch: &Launch) -> Result<()> {
    ensure!(
        launch.command.is_file(),
        "Executable missing: {}",
        launch.command.display()
    );
    ensure!(
        !launch
            .command
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")),
        "Choose the native executable, not a .cmd/.bat wrapper. Node tools are resolved through their package metadata."
    );
    if launch.args.first().is_some_and(|a| {
        Path::new(a).extension().is_some_and(|e| e == "js") || a.contains("node_modules")
    }) {
        ensure!(
            Path::new(&launch.args[0]).is_file(),
            "Node entry script is missing"
        );
    }
    Ok(())
}
fn installed_launch(entry: &Entry, dir: &Path) -> Result<Launch> {
    match entry.method {
        Method::Npm(p) => npm_launch(dir, p, entry.executable),
        Method::Python(_) => Ok(Launch {
            command: dir.join(if cfg!(windows) {
                "Scripts/ruff.exe"
            } else {
                "bin/ruff"
            }),
            args: vec![],
        }),
        Method::GithubClang => {
            let executable = managed::find_executable(
                dir,
                if cfg!(windows) {
                    "clangd.exe"
                } else {
                    "clangd"
                },
            )?;
            Ok(Launch {
                command: executable,
                args: vec![],
            })
        }
        Method::GithubRust => Ok(Launch {
            command: dir.join(if cfg!(windows) {
                "rust-analyzer.exe"
            } else {
                "rust-analyzer"
            }),
            args: vec![],
        }),
    }
}
fn detect_version(
    entry: &Entry,
    dir: Option<&Path>,
    launch: &Launch,
    cancel: &AtomicBool,
) -> Result<String> {
    if let (Some(dir), Method::Npm(p)) = (dir, entry.method) {
        let value: Value = serde_json::from_slice(&std::fs::read(
            dir.join("node_modules").join(p).join("package.json"),
        )?)?;
        return Ok(value["version"]
            .as_str()
            .context("Installed package has no version")?
            .into());
    }
    let mut c = command(&launch.command, &launch.args);
    c.arg("--version");
    Ok(run(c, cancel, 5)?.trim().chars().take(200).collect())
}
fn safe_directory(data: &Path, id: &str, dir: &Path) -> Result<()> {
    ensure!(lsp_catalog::entry(id).is_some(), "Unknown managed tool");
    let packages = data.join("packages").join(id).canonicalize()?;
    let path = dir.canonicalize()?;
    ensure!(
        path.parent() == Some(packages.as_path()),
        "Refusing to delete a path outside this tool's generation directory"
    );
    ensure!(
        !std::fs::symlink_metadata(dir)?.file_type().is_symlink(),
        "Managed directory is a symlink"
    );
    ensure!(
        std::fs::read_to_string(path.join(".filavim-owned"))? == id,
        "Installation is not owned by Filavim"
    );
    Ok(())
}
fn install(
    data: &Path,
    entry: &Entry,
    store: &mut Store,
    cancel: &AtomicBool,
    report: &impl Fn(&str),
) -> Result<()> {
    ensure!(
        entry.supports_host(),
        "This server is not supported on this OS/architecture by the curated installer"
    );
    let (version, release) = available(entry, cancel)?;
    report(&format!("Preparing {} {version}", entry.id));
    let parent = data.join("packages").join(entry.id);
    std::fs::create_dir_all(&parent)?;
    // Python venv launchers are not relocatable: create in final generation path,
    // then atomically publish only the metadata pointer after validation.
    let staging = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&parent)?;
    std::fs::write(staging.path().join(".filavim-owned"), entry.id)?;
    match entry.method {
        Method::Npm(package) => {
            let (node, cli) = managed::node(data, cancel, report)?;
            std::fs::write(
                staging.path().join(".filavim-node.json"),
                serde_json::to_vec(&node)?,
            )?;
            let mut check_node = Command::new(&node);
            check_node
                .arg("--version")
                .env_remove("NODE_OPTIONS")
                .env_remove("NODE_PATH");
            let node_version = run(check_node, cancel, 5)?;
            ensure!(
                node_version
                    .trim()
                    .trim_start_matches('v')
                    .split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|n| n >= 20),
                "Node.js >=20 is required"
            );
            report(
                "Downloading isolated npm package (integrity checked by npm; lifecycle scripts disabled)",
            );
            let cache = tempfile::tempdir_in(data)?;
            let mut c = Command::new(&node);
            c.arg(&cli).arg("--cache").arg(cache.path());
            managed::isolate_npm(&mut c, staging.path())?;
            c.current_dir(staging.path())
                .args(["install", "--package-lock-only", "--prefix"])
                .arg(staging.path())
                .args([
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--registry=https://registry.npmjs.org",
                    "--",
                    &format!("{package}@{version}"),
                ]);
            if entry.id == "typescript-language-server" {
                c.arg("typescript@6");
            }
            let started = Instant::now();
            run_with_tick(c, cancel, 240, || {
                report(&format!(
                    "Installing verified npm package · {} s elapsed",
                    started.elapsed().as_secs()
                ))
            })?;
            managed::validate_lock(staging.path())?;
            let mut install = Command::new(&node);
            install
                .arg(&cli)
                .arg("--cache")
                .arg(cache.path())
                .current_dir(staging.path())
                .args([
                    "ci",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--registry=https://registry.npmjs.org",
                ]);
            managed::isolate_npm(&mut install, staging.path())?;
            run_with_tick(install, cancel, 240, || {
                report("Installing integrity-verified locked packages")
            })?;
        }
        Method::Python(package) => {
            let (python, args) = python()?;
            report("Creating isolated Python environment");
            let mut c = command(&python, &args);
            c.args(["-m", "venv"]).arg(staging.path());
            run(c, cancel, 60)?;
            report("Downloading binary wheel from PyPI (no source builds)");
            let hashes = release["urls"]
                .as_array()
                .context("PyPI returned no wheels")?
                .iter()
                .filter(|v| v["packagetype"] == "bdist_wheel")
                .filter_map(|v| v["digests"]["sha256"].as_str())
                .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(|h| format!(" --hash=sha256:{h}"))
                .collect::<String>();
            ensure!(
                !hashes.is_empty(),
                "PyPI did not provide wheel SHA-256 digests"
            );
            let requirements = staging.path().join("requirements.txt");
            std::fs::write(&requirements, format!("{package}=={version}{hashes}\n"))?;
            let mut c = Command::new(staging.path().join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            }));
            c.args([
                "-m",
                "pip",
                "--isolated",
                "install",
                "--disable-pip-version-check",
                "--no-cache-dir",
                "--only-binary=:all:",
                "--index-url=https://pypi.org/simple",
                "--require-hashes",
                "-r",
            ])
            .arg(requirements);
            let started = Instant::now();
            run_with_tick(c, cancel, 240, || {
                report(&format!(
                    "Installing SHA-256-verified wheel · {} s elapsed",
                    started.elapsed().as_secs()
                ))
            })?;
        }
        Method::GithubClang => managed::install_clang(&release, staging.path(), cancel, report)?,
        Method::GithubRust => {
            let arch = match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "aarch64",
                _ => bail!("rust-analyzer downloads support x86_64/aarch64 only"),
            };
            let target = match std::env::consts::OS {
                "windows" => "pc-windows-msvc.zip",
                "macos" => "apple-darwin.gz",
                "linux" => "unknown-linux-gnu.gz",
                _ => bail!("Unsupported rust-analyzer platform"),
            };
            let name = format!("rust-analyzer-{arch}-{target}");
            let asset = release["assets"]
                .as_array()
                .and_then(|a| a.iter().find(|v| v["name"] == name))
                .context("Release has no executable for this OS/architecture")?;
            let url = asset["browser_download_url"]
                .as_str()
                .context("Missing download URL")?;
            ensure!(
                url.starts_with("https://github.com/rust-lang/rust-analyzer/releases/download/"),
                "Unexpected release download host/path"
            );
            let digest = asset["digest"]
                .as_str()
                .and_then(|s| s.strip_prefix("sha256:"))
                .context("No publisher asset SHA-256 available; refusing unverified executable")?;
            ensure!(
                digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid SHA-256 digest"
            );
            let archive = staging.path().join("download");
            report("Downloading official executable and verifying SHA-256");
            download_progress(url, &archive, cancel, 128 * 1024 * 1024, report)?;
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            let mut f = std::fs::File::open(&archive)?;
            let mut buf = [0; 65536];
            loop {
                check(cancel)?;
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
            }
            ensure!(
                format!("{:x}", hash.finalize()).eq_ignore_ascii_case(digest),
                "Download SHA-256 mismatch"
            );
            let output = staging.path().join(if cfg!(windows) {
                "rust-analyzer.exe"
            } else {
                "rust-analyzer"
            });
            if cfg!(windows) {
                let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive)?)?;
                let mut member = zip.by_name("rust-analyzer.exe")?;
                ensure!(
                    member.size() <= 256 * 1024 * 1024,
                    "Uncompressed executable exceeds limit"
                );
                std::io::copy(&mut member, &mut std::fs::File::create(&output)?)?;
            } else {
                let mut gzip = flate2::read::GzDecoder::new(std::fs::File::open(&archive)?)
                    .take(256 * 1024 * 1024 + 1);
                let size = std::io::copy(&mut gzip, &mut std::fs::File::create(&output)?)?;
                ensure!(
                    size <= 256 * 1024 * 1024,
                    "Uncompressed executable exceeds limit"
                );
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700))?;
                }
            }
            std::fs::remove_file(archive)?;
        }
    }
    check(cancel)?;
    report("Validating executable and launch prerequisites");
    let mut launch = installed_launch(entry, staging.path())?;
    validate_launch(&launch)?;
    let actual = detect_version(entry, Some(staging.path()), &launch, cancel)?;
    if matches!(entry.method, Method::Npm(_)) {
        ensure!(
            actual == version,
            "Installed version differs from pinned source"
        );
    }
    if entry.id == "typescript-native" {
        let mut c = command(&launch.command, &launch.args);
        c.arg("--version");
        ensure!(
            run(c, cancel, 5)?.contains(&version),
            "Native TypeScript executable failed version verification"
        );
    }
    launch.args.extend(entry.args.iter().map(|s| s.to_string()));
    report("Checking LSP initialize / shutdown in an empty temporary workspace");
    probe(entry, &launch, cancel)?;
    check(cancel)?;
    let directory = staging.keep();
    let old = store.installed.insert(
        entry.id.into(),
        Installation {
            directory: directory.clone(),
            version,
            launch,
            source: entry.source.into(),
        },
    );
    let previous = old
        .as_ref()
        .and_then(|old| store.previous.insert(entry.id.into(), old.clone()));
    if let Err(e) = save(data, store) {
        let _ = std::fs::remove_dir_all(&directory);
        if let Some(old) = old {
            store.installed.insert(entry.id.into(), old);
            if let Some(previous) = previous {
                store.previous.insert(entry.id.into(), previous);
            } else {
                store.previous.remove(entry.id);
            }
        } else {
            store.installed.remove(entry.id);
        }
        return Err(e);
    }
    // An old generation can still be used by a process. Keep it until explicit
    // uninstall rather than deleting files from beneath a running server.
    Ok(())
}
fn resolve(
    entry: &Entry,
    root: &Path,
    data: &Path,
    store: &Store,
    prefs: &Preferences,
    cancel: &AtomicBool,
) -> Status {
    let result = (|| -> Result<(Launch, String, LaunchOrigin)> {
        ensure!(
            entry.supports_host(),
            "Unsupported OS/architecture for this catalog entry"
        );
        if let Some(custom) = prefs.custom.get(entry.id) {
            validate_launch(custom)?;
            return Ok((
                custom.clone(),
                "Custom executable (version not probed)".into(),
                LaunchOrigin::Configured,
            ));
        }
        // Project-local tools are never modified or removed by the manager.
        if let Method::Npm(p) = entry.method
            && root
                .join("node_modules")
                .join(p)
                .join("package.json")
                .is_file()
        {
            let mut launch = npm_launch(root, p, entry.executable)?;
            let v = detect_version(entry, Some(root), &launch, cancel)?;
            if entry.id == "typescript-native" {
                ensure!(
                    v.split('.')
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .is_some_and(|n| n >= 7),
                    "Project TypeScript is older than 7; select vtsls"
                );
            }
            launch.args.extend(entry.args.iter().map(|s| s.to_string()));
            return Ok((launch, v, LaunchOrigin::Workspace));
        }
        if let Some(i) = store.installed.get(entry.id) {
            safe_directory(data, entry.id, &i.directory)?;
            validate_launch(&i.launch)?;
            return Ok((i.launch.clone(), i.version.clone(), LaunchOrigin::Managed));
        }
        if matches!(entry.method, Method::Python(_)) {
            for environment in [".venv", "venv"] {
                let executable = root.join(environment).join(if cfg!(windows) {
                    format!("Scripts/{}.exe", entry.executable)
                } else {
                    format!("bin/{}", entry.executable)
                });
                if executable.is_file() {
                    let mut launch = Launch {
                        command: executable,
                        args: vec![],
                    };
                    let v = detect_version(entry, None, &launch, cancel)?;
                    launch.args.extend(entry.args.iter().map(|s| s.to_string()));
                    return Ok((launch, v, LaunchOrigin::Workspace));
                }
            }
        }
        let path = tool(entry.executable)?;
        let (mut launch, v) = if let Method::Npm(package) = entry.method {
            let real = path.canonicalize()?;
            let directory = real.ancestors().take(7).find(|p| {
                std::fs::read(p.join("package.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|v| v["name"] == package)
            });
            if let Some(directory) = directory {
                let value: Value =
                    serde_json::from_slice(&std::fs::read(directory.join("package.json"))?)?;
                (
                    package_launch(directory, entry.executable)?,
                    value["version"]
                        .as_str()
                        .context("No package version")?
                        .to_owned(),
                )
            } else {
                let parent = path.parent().context("No executable parent")?;
                let launch = npm_launch(parent, package, entry.executable)
                    .context("Cannot resolve system Node wrapper; use an isolated managed copy")?;
                let version = detect_version(entry, Some(parent), &launch, cancel)?;
                (launch, version)
            }
        } else {
            let launch = Launch {
                command: path,
                args: vec![],
            };
            let v = detect_version(entry, None, &launch, cancel)?;
            (launch, v)
        };
        if entry.id == "typescript-native" {
            ensure!(
                v.split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|n| n >= 7),
                "System tsc is older than TypeScript 7"
            );
        }
        launch.args.extend(entry.args.iter().map(|s| s.to_string()));
        Ok((launch, v, LaunchOrigin::System))
    })();
    match result {
        Ok((launch, version, origin)) => Status {
            launch: Some(launch),
            version: Some(version),
            managed: origin == LaunchOrigin::Managed,
            origin: Some(origin),
            ..Status::default()
        },
        Err(e) => Status {
            managed: store.installed.contains_key(entry.id),
            version: store
                .installed
                .get(entry.id)
                .map(|receipt| receipt.version.clone()),
            warning: Some(format!("{e:#}")),
            ..Status::default()
        },
    }
}
pub fn selected_for(entry: &Entry, language: &str, prefs: &Preferences) -> bool {
    if prefs.all_disabled == Some(true)
        || prefs.disabled.contains(entry.id)
        || !entry.languages.contains(&language)
    {
        return false;
    }
    entry.complementary
        || prefs.primary.get(language).map_or_else(
            || {
                lsp_catalog::CATALOG
                    .iter()
                    .find(|e| {
                        !e.complementary
                            && e.languages.contains(&language)
                            && prefs.custom.contains_key(e.id)
                            && !prefs.disabled.contains(e.id)
                    })
                    .or_else(|| lsp_catalog::primary(language))
                    .is_some_and(|p| p.id == entry.id)
            },
            |id| id == entry.id,
        )
}
pub fn selected(entry: &Entry, prefs: &Preferences) -> bool {
    entry
        .languages
        .iter()
        .any(|language| selected_for(entry, language, prefs))
}

fn scan(
    data: &Path,
    workspace: &Path,
    paths: &BTreeSet<PathBuf>,
    store: Store,
    cancel: &AtomicBool,
) -> Result<Snapshot> {
    let prefs = store.preferences(workspace);
    let canonical_workspace = workspace.canonicalize().ok();
    let trusted = canonical_workspace.as_ref().is_some_and(|root| {
        let key = crate::lsp::path_key(root);
        store.trusted_workspaces.get(&key).is_some_and(|expected| {
            crate::lsp::workspace::directory_identity(root)
                .is_ok_and(|identity| identity == Some(*expected))
        })
    });
    let mut snapshot = Snapshot {
        store: store.clone(),
        trusted,
        ..Snapshot::default()
    };
    for entry in lsp_catalog::CATALOG {
        check(cancel)?;
        let mut status = if trusted {
            resolve(entry, workspace, data, &store, &prefs, cancel)
        } else {
            untrusted_status(entry, data, &store)
        };
        status.dependency_warning = if trusted {
            match entry.method {
                Method::Npm(_) => None,
                Method::Python(_) => python()
                    .err()
                    .map(|e| format!("Installation dependency: {e:#}")),
                Method::GithubClang => None,
                Method::GithubRust => {
                    if crate::setup::executable("curl").is_none() {
                        Some("curl is required for official release downloads".into())
                    } else if crate::setup::executable("cargo").is_none() {
                        Some(
                            "Cargo/Rust toolchain is unavailable; project analysis will be limited"
                                .into(),
                        )
                    } else {
                        None
                    }
                }
            }
        } else {
            None
        };
        status.available = store.latest.get(entry.id).map(|v| v.version.clone());
        snapshot.statuses.insert(entry.id.into(), status);
    }
    let Some(workspace_root) = canonical_workspace.filter(|_| trusted) else {
        return Ok(snapshot);
    };
    let workspace_root = PathBuf::from(crate::paths::display_path(&workspace_root));
    for path in paths {
        let Ok(canonical_path) = crate::files::resolve_path(path) else {
            continue;
        };
        let canonical_path = PathBuf::from(crate::paths::display_path(&canonical_path));
        if crate::lsp::workspace::relative_path(&canonical_path, &workspace_root).is_none() {
            continue;
        }
        snapshot.trusted_paths.insert(crate::lsp::path_key(path));
        let Some(language) = crate::lsp::language(path) else {
            continue;
        };
        for entry in lsp_catalog::CATALOG
            .iter()
            .filter(|e| selected_for(e, language, &prefs))
        {
            let root = lsp_catalog::root(&canonical_path, entry);
            let root = if crate::lsp::workspace::relative_path(&root, &workspace_root).is_some() {
                root
            } else {
                workspace_root.clone()
            };
            let status = if crate::lsp::path_key(&root) == crate::lsp::path_key(workspace) {
                snapshot.statuses[entry.id].clone()
            } else {
                resolve(entry, &root, data, &store, &prefs, cancel)
            };
            if snapshot
                .statuses
                .get(entry.id)
                .is_none_or(|s| s.launch.is_none())
                && status.launch.is_some()
            {
                snapshot.statuses.insert(entry.id.into(), status.clone());
            }
            if let Some(launch) = status.launch {
                snapshot
                    .resolved
                    .entry(crate::lsp::path_key(path))
                    .or_default()
                    .push(Resolved {
                        id: entry.id.into(),
                        root,
                        config: ServerConfig {
                            name: entry.id.into(),
                            command: launch.command.to_string_lossy().into_owned(),
                            args: launch.args,
                            languages: entry.languages.iter().map(|s| s.to_string()).collect(),
                            initialization_options: entry.initialization_options(),
                            settings: prefs
                                .settings
                                .get(entry.id)
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({})),
                            ..ServerConfig::default()
                        },
                    });
            } else if !prefs.dismissed.contains(entry.id) {
                snapshot.missing.insert(entry.id.into());
            }
        }
    }
    Ok(snapshot)
}

fn untrusted_status(entry: &Entry, data: &Path, store: &Store) -> Status {
    let Some(installation) = store.installed.get(entry.id) else {
        return Status::default();
    };
    let valid = safe_directory(data, entry.id, &installation.directory)
        .and_then(|()| validate_launch(&installation.launch));
    Status {
        launch: valid.is_ok().then(|| installation.launch.clone()),
        version: Some(installation.version.clone()),
        managed: true,
        origin: Some(LaunchOrigin::Managed),
        warning: valid.err().map(|error| format!("{error:#}")),
        ..Status::default()
    }
}
fn cleanup(data: &Path, store: &Store, cancel: &AtomicBool, report: &impl Fn(&str)) -> Result<()> {
    let mut retained = BTreeSet::new();
    if let Ok(pointer) = std::fs::read(data.join("runtimes/node/current.json"))
        && let Ok(node) = serde_json::from_slice::<PathBuf>(&pointer)
    {
        for parent in node.ancestors() {
            if parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("generation-"))
                && parent.exists()
            {
                retained.insert(parent.canonicalize()?);
                break;
            }
        }
    }
    for installation in store.installed.values().chain(store.previous.values()) {
        retained.insert(installation.directory.canonicalize()?);
        for parent in installation.launch.command.ancestors() {
            if parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("generation-"))
            {
                retained.insert(parent.canonicalize()?);
                break;
            }
        }
    }
    let mut roots: Vec<_> = lsp_catalog::CATALOG
        .iter()
        .map(|entry| (data.join("packages").join(entry.id), entry.id))
        .collect();
    roots.push((data.join("runtimes/node"), "node"));
    for (root, owner) in roots {
        if !root.exists() {
            continue;
        }
        let root = root.canonicalize()?;
        for entry in std::fs::read_dir(&root)? {
            check(cancel)?;
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                continue;
            }
            let canonical = path.canonicalize()?;
            ensure!(
                canonical.parent() == Some(root.as_path()),
                "Cleanup target escaped generation root"
            );
            if retained.contains(&canonical)
                || std::fs::read_to_string(path.join(".filavim-owned"))
                    .ok()
                    .as_deref()
                    != Some(owner)
            {
                continue;
            }
            let lease = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(generation_lease_path(&path)?)?;
            if lease.try_lock().is_err() {
                report("Generation is in use; retained for later cleanup");
                continue;
            }
            // Rename while the lease is held prevents new clients from opening it.
            let garbage = tempfile::Builder::new()
                .prefix("cleanup-")
                .tempdir_in(&root)?;
            let destination = garbage.path().join("generation");
            std::fs::rename(&path, &destination)?;
            drop(lease);
            garbage.close()?;
        }
    }
    Ok(())
}

/// Hold managed generations against cleanup for the lifetime of a server process.
pub(crate) fn generation_leases(command: &Path, args: &[String]) -> Result<Vec<std::fs::File>> {
    let mut directories = BTreeSet::new();
    for path in std::iter::once(command).chain(args.iter().map(Path::new)) {
        for parent in path.ancestors() {
            if parent.join(".filavim-owned").is_file() {
                directories.insert(parent.to_path_buf());
                break;
            }
        }
    }
    directories
        .into_iter()
        .map(|directory| {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(generation_lease_path(&directory)?)?;
            file.try_lock_shared()
                .context("Managed generation is being removed; refresh language tools")?;
            Ok(file)
        })
        .collect()
}

fn generation_lease_path(directory: &Path) -> Result<PathBuf> {
    let name = directory
        .file_name()
        .context("Generation has no name")?
        .to_string_lossy();
    Ok(directory
        .parent()
        .context("Generation has no parent")?
        .join(format!(".{name}.lease")))
}

fn execute(
    data: &Path,
    workspace: &Path,
    paths: &BTreeSet<PathBuf>,
    operation: Operation,
    cancel: &AtomicBool,
    report: &impl Fn(&str),
) -> Result<Snapshot> {
    let updates = matches!(operation, Operation::CheckUpdates);
    let _lock = if !matches!(operation, Operation::Refresh) {
        std::fs::create_dir_all(data)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data.join("manager.lock"))?;
        file.try_lock().map_err(|e| {
            anyhow::anyhow!(
                "Another Filavim instance is managing language tools; retry after it finishes: {e}"
            )
        })?;
        Some(file)
    } else {
        None
    };
    let mut store = load(data)?;
    match operation {
        Operation::RepairRuntime => {
            let old_pointer = std::fs::read(data.join("runtimes/node/current.json")).ok();
            let (node, _) = managed::download_node(data, cancel, report)?;
            let result = (|| -> Result<()> {
                for (id, installation) in
                    store.installed.iter_mut().chain(store.previous.iter_mut())
                {
                    if !matches!(
                        lsp_catalog::entry(id).context("Unknown receipt")?.method,
                        Method::Npm(_)
                    ) {
                        continue;
                    }
                    safe_directory(data, id, &installation.directory)?;
                    installation.launch.command = node.clone();
                    probe(
                        lsp_catalog::entry(id).unwrap(),
                        &installation.launch,
                        cancel,
                    )?;
                }
                check(cancel)?;
                save(data, &store)
            })();
            if let Err(error) = result {
                if let Some(pointer) = old_pointer {
                    crate::files::save(
                        &data.join("runtimes/node/current.json"),
                        &ropey::Rope::from_str(std::str::from_utf8(&pointer)?),
                    )?;
                }
                return Err(error).context("Runtime repair failed; server receipts preserved");
            }
        }
        Operation::Cleanup => cleanup(data, &store, cancel, report)?,
        Operation::TrustWorkspace(trusted) => {
            let root = workspace
                .canonicalize()
                .context("Cannot resolve workspace to set trust")?;
            ensure!(root.is_dir(), "Only a project directory can be trusted");
            let key = crate::lsp::path_key(&root);
            if trusted {
                let identity = crate::lsp::workspace::directory_identity(&root)?
                    .context("Workspace directory identity is unavailable")?;
                store.trusted_workspaces.insert(key, identity);
            } else {
                store.trusted_workspaces.remove(&key);
            }
            save(data, &store)?;
        }
        Operation::Install(id) => install(
            data,
            lsp_catalog::entry(&id).context("Unknown server")?,
            &mut store,
            cancel,
            report,
        )?,
        Operation::Rollback(id) => {
            let entry = lsp_catalog::entry(&id).context("Unknown server")?;
            let previous = store
                .previous
                .get(&id)
                .cloned()
                .context("No previous managed installation")?;
            safe_directory(data, &id, &previous.directory)?;
            validate_launch(&previous.launch)?;
            report("Checking the previous version in an empty temporary workspace");
            probe(entry, &previous.launch, cancel)?;
            check(cancel)?;
            let active = store
                .installed
                .insert(id.clone(), previous)
                .context("No active managed installation")?;
            store.previous.insert(id, active);
            save(data, &store)?;
        }
        Operation::Remove(id) => {
            let i = store
                .installed
                .get(&id)
                .context("Only Filavim-owned installations can be removed")?;
            if i.directory.exists() {
                safe_directory(data, &id, &i.directory)?;
            }
            store.installed.remove(&id);
            store.previous.remove(&id);
            save(data, &store)?;
            cleanup(data, &store, cancel, report)?;
        }
        Operation::Configure {
            workspace: local,
            change,
        } => {
            let p = if local {
                store
                    .workspaces
                    .entry(crate::lsp::path_key(workspace))
                    .or_default()
            } else {
                &mut store.global
            };
            match change {
                Change::Enable(id, on) => {
                    ensure!(lsp_catalog::entry(&id).is_some(), "Unknown server");
                    if on {
                        p.disabled.remove(&id);
                        p.enabled.insert(id);
                    } else {
                        p.enabled.remove(&id);
                        p.disabled.insert(id);
                    }
                }
                Change::Select(id) => {
                    let e = lsp_catalog::entry(&id).context("Unknown server")?;
                    ensure!(
                        !e.complementary,
                        "Complementary tools do not replace primary servers"
                    );
                    for l in e.languages {
                        p.primary.insert((*l).into(), id.clone());
                    }
                    p.disabled.remove(&id);
                    p.enabled.insert(id);
                }
                Change::Custom(id, launch) => {
                    ensure!(lsp_catalog::entry(&id).is_some(), "Unknown server");
                    validate_launch(&launch)?;
                    p.custom.insert(id, launch);
                }
                Change::Settings(id, settings) => {
                    ensure!(lsp_catalog::entry(&id).is_some(), "Unknown server");
                    ensure!(settings.is_object(), "Server settings must be an object");
                    p.settings.insert(id, settings);
                }
                Change::Dismiss(ids) => {
                    p.dismissed.extend(ids);
                }
                Change::All(disabled) => p.all_disabled = Some(disabled),
            }
            save(data, &store)?;
        }
        _ => {}
    }
    let mut snapshot = scan(data, workspace, paths, store, cancel)?;
    if updates {
        for e in lsp_catalog::CATALOG {
            check(cancel)?;
            report(&format!("Checking {} stable version", e.id));
            match available(e, cancel) {
                Ok((v, _)) => {
                    snapshot.store.latest.insert(
                        e.id.into(),
                        CachedVersion {
                            version: v.clone(),
                            checked_unix: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)?
                                .as_secs(),
                        },
                    );
                    snapshot.statuses.entry(e.id.into()).or_default().available = Some(v);
                }
                Err(error) => {
                    snapshot.statuses.entry(e.id.into()).or_default().warning =
                        Some(format!("Update check failed: {error:#}"))
                }
            }
        }
    }
    if updates {
        save(data, &snapshot.store)?;
    }
    Ok(snapshot)
}

fn probe(entry: &Entry, launch: &Launch, cancel: &AtomicBool) -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = crate::lsp::Client::start(
        ServerConfig {
            name: entry.id.into(),
            command: launch.command.to_string_lossy().into_owned(),
            args: launch.args.clone(),
            languages: entry.languages.iter().map(|s| s.to_string()).collect(),
            initialization_options: entry.initialization_options(),
            ..ServerConfig::default()
        },
        root.path().into(),
        Duration::from_secs(15),
    );
    let result = (|| {
        let started = Instant::now();
        loop {
            check(cancel)?;
            while let Ok(event) = client.events.try_recv() {
                match event {
                    crate::lsp::Event::Ready { .. } => return Ok(()),
                    crate::lsp::Event::Stopped(error) => bail!(
                        "Installed server failed initialization: {error}\n{}",
                        client.stderr()
                    ),
                    crate::lsp::Event::ServerRequest { id, method, params } => client.respond(
                        id,
                        if method == "workspace/configuration" {
                            Value::Array(
                                params["items"]
                                    .as_array()
                                    .map_or_else(Vec::new, |items| vec![Value::Null; items.len()]),
                            )
                        } else {
                            Value::Null
                        },
                    ),
                    _ => {}
                }
            }
            ensure!(
                started.elapsed() < Duration::from_secs(18),
                "Installed server did not initialize within 18 seconds\n{}",
                client.stderr()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    client.shutdown();
    let stop = Instant::now();
    while !client.stopped.load(Ordering::Acquire) && stop.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    #[ignore = "downloads real JSON server and Node runtime in an isolated directory"]
    fn real_runtime_repair_preserves_server_and_rollback_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        let data = temp.path().join("tools");
        let cancel = AtomicBool::new(false);
        let report = |message: &str| eprintln!("{message}");
        for _ in 0..2 {
            execute(
                &data,
                &workspace,
                &BTreeSet::new(),
                Operation::Install("json-language-server".into()),
                &cancel,
                &report,
            )
            .unwrap();
        }
        let before = load(&data).unwrap();
        let package = before.installed["json-language-server"].directory.clone();
        execute(
            &data,
            &workspace,
            &BTreeSet::new(),
            Operation::RepairRuntime,
            &cancel,
            &report,
        )
        .unwrap();
        let repaired = load(&data).unwrap();
        assert_eq!(
            repaired.installed["json-language-server"].directory,
            package
        );
        assert_ne!(
            repaired.installed["json-language-server"].launch.command,
            before.installed["json-language-server"].launch.command
        );
        assert_eq!(
            repaired.previous["json-language-server"].launch.command,
            repaired.installed["json-language-server"].launch.command
        );
        execute(
            &data,
            &workspace,
            &BTreeSet::new(),
            Operation::Rollback("json-language-server".into()),
            &cancel,
            &report,
        )
        .unwrap();
        execute(
            &data,
            &workspace,
            &BTreeSet::new(),
            Operation::Remove("json-language-server".into()),
            &cancel,
            &report,
        )
        .unwrap();
        assert!(load(&data).unwrap().installed.is_empty());
        assert!(!package.exists());
    }
    #[test]
    fn cleanup_preserves_receipts_and_process_leases() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packages/rust-analyzer");
        let active = root.join("generation-active");
        let unused = root.join("generation-unused");
        let leased = root.join("generation-leased");
        for path in [&active, &unused, &leased] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join(".filavim-owned"), "rust-analyzer").unwrap();
            std::fs::write(path.join("server"), "fixture").unwrap();
        }
        let mut store = Store::default();
        store.installed.insert(
            "rust-analyzer".into(),
            Installation {
                directory: active.clone(),
                version: "fixture".into(),
                source: "fixture".into(),
                launch: Launch {
                    command: active.join("server"),
                    args: Vec::new(),
                },
            },
        );
        let leases = generation_leases(&leased.join("server"), &[]).unwrap();
        cleanup(temp.path(), &store, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(active.exists());
        assert!(leased.exists());
        assert!(!unused.exists());
        drop(leases);
        cleanup(temp.path(), &store, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(!leased.exists());
        assert!(active.exists());
    }

    fn real_editor_navigation(
        services: &mut crate::gui::services::Services,
        docs: &mut crate::gui::documents::Documents,
        id: &str,
        capabilities: &Value,
    ) {
        use crate::gui::services::Kind;
        let source = match id {
            "rust-analyzer" => {
                "fn greet(value:i32)->i32{value+1}\nfn main(){let _result=greet(1);}\n"
            }
            "clangd" => "int greet(int value){return value+1;}\nint main(){return greet(1);}\n",
            "typescript-language-server" => {
                "function greet(value:number){return value+1;}\nconst result=greet(1);\n"
            }
            "pyright" => "def greet(value: int) -> int:\n    return value+1\nresult=greet(1)\n",
            "json-language-server" => "{\"$schema\":\"./schema.json\",\"property\":\"name\"}\n",
            _ => return,
        };
        let path = docs.editor.buffer.path.clone().unwrap();
        let disk = std::fs::read(&path).unwrap();
        docs.editor.snippet = None;
        docs.editor.set_mode(crate::editor::Mode::Normal);
        let end = docs.editor.buffer.text.len_chars();
        docs.editor.buffer.replace(0..end, source);
        let symbol = if id == "json-language-server" {
            "property"
        } else {
            "greet"
        };
        let symbol_at = source.rfind(symbol).unwrap();

        for (capability, kind, position, params) in [
            ("hoverProvider", Kind::Hover, symbol_at + 1, None),
            (
                "signatureHelpProvider",
                Kind::Signature,
                symbol_at + symbol.len() + 1,
                None,
            ),
            ("referencesProvider", Kind::References, symbol_at + 1, None),
            (
                "documentSymbolProvider",
                Kind::DocumentSymbols,
                symbol_at + 1,
                None,
            ),
            (
                "workspaceSymbolProvider",
                Kind::WorkspaceSymbols,
                symbol_at + 1,
                Some(json!({"query":symbol})),
            ),
            (
                "renameProvider",
                Kind::Rename,
                symbol_at + 1,
                Some(json!({"newName":"welcome"})),
            ),
            ("documentFormattingProvider", Kind::Formatting, 0, None),
        ] {
            if capabilities[capability].is_null() || capabilities[capability] == false {
                continue;
            }
            services.dismiss();
            docs.editor.buffer.cursor = position;
            services.update(docs);
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut next_request = Instant::now();
            let verified = loop {
                services.poll(docs);
                if services.panel.as_ref().is_some_and(|p| {
                    p.kind == kind
                        && p.rows
                            .iter()
                            .any(|row| !row.label.is_empty() && row.label != "No information")
                }) {
                    break true;
                }
                let now = Instant::now();
                if now >= deadline {
                    break false;
                }
                if now >= next_request {
                    services.request(docs, kind, params.clone());
                    next_request = now + Duration::from_millis(500);
                }
                std::thread::sleep(Duration::from_millis(15));
            };
            if !verified {
                eprintln!(
                    "REAL EXTENDED {id} {}: advertised, no usable result for this fixture ({:?})",
                    kind.method(),
                    services.message
                );
                continue;
            }
            if matches!(kind, Kind::Rename | Kind::Formatting) {
                let revision = docs.editor.buffer.revision;
                services.accept(docs);
                let deadline = Instant::now() + Duration::from_secs(8);
                while services.workspace_active() && Instant::now() < deadline {
                    services.poll(docs);
                    std::thread::sleep(Duration::from_millis(15));
                }
                assert!(
                    !services.workspace_active(),
                    "{id}: {} edit did not settle",
                    kind.method()
                );
                if docs.editor.buffer.revision == revision {
                    eprintln!(
                        "REAL EXTENDED {id} {}: no buffer change ({:?})",
                        kind.method(),
                        services.message
                    );
                    continue;
                }
                if kind == Kind::Rename {
                    assert_eq!(
                        docs.editor.buffer.text.to_string(),
                        source.replace("greet", "welcome")
                    );
                }
                assert_eq!(std::fs::read(&path).unwrap(), disk);
                docs.editor.buffer.undo();
                assert_eq!(docs.editor.buffer.text.to_string(), source);
            }
            eprintln!("REAL EXTENDED {id} {}: verified", kind.method());
        }
    }
    fn real_editor_completion(
        data: &Path,
        workspace: &Path,
        id: &str,
        file: &str,
        text: &str,
        expected: &str,
    ) {
        use crate::gui::{
            documents::Documents,
            model::Model,
            services::{Kind, Services},
        };
        if !lsp_catalog::entry(id).unwrap().recommended {
            execute(
                data,
                workspace,
                &BTreeSet::new(),
                Operation::Configure {
                    workspace: false,
                    change: Change::Select(id.into()),
                },
                &AtomicBool::new(false),
                &|_| {},
            )
            .unwrap();
        }
        let path = workspace.join(file);
        std::fs::write(&path, text).unwrap();
        let mut docs = Documents::new(
            Model::new(crate::config::Config {
                syntax: false,
                ..Default::default()
            })
            .unwrap(),
        );
        docs.add(
            crate::buffer::Buffer::new(ropey::Rope::from_str(text), Some(path)),
            true,
        )
        .unwrap();
        docs.editor.set_mode(crate::editor::Mode::Insert);
        docs.editor.buffer.cursor = text.chars().count();
        let mut services = Services::new(workspace.into());
        services.manager = Manager::at(data.into(), workspace.into());
        execute(
            data,
            workspace,
            &BTreeSet::new(),
            Operation::TrustWorkspace(true),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let started = Instant::now();
        let mut request = Instant::now() - Duration::from_secs(2);
        loop {
            services.poll(&mut docs);
            services.update(&docs);
            if services.panel.as_ref().is_some_and(|p| {
                p.kind == Kind::Completion && p.rows.iter().any(|r| r.label.starts_with(expected))
            }) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(35),
                "{id} did not return {expected} completion: {:?}",
                services.runtime(id)
            );
            if services.runtime(id).iter().any(|s| s.state == "Running")
                && request.elapsed() > Duration::from_millis(500)
            {
                services.request(&docs, Kind::Completion, None);
                request = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "REAL EDITOR {id}: received expected {expected} completion through Services/popup preparation"
        );
        // A real server list must remain usable after another typed character,
        // including its original LSP replacement range and any resolve response.
        let session = services.panel.as_ref().unwrap().request;
        let prefix = crate::gui::completion::prefix(&docs.editor.buffer);
        let query = docs
            .editor
            .buffer
            .text
            .slice(prefix..docs.editor.buffer.cursor)
            .to_string();
        if let Some(c) = expected.chars().nth(query.chars().count()) {
            docs.editor.type_character(c);
            services.update(&docs);
            services.automatic_completion(&docs);
            assert_eq!(services.panel.as_ref().unwrap().request, session);
        }
        let panel = services.panel.as_mut().unwrap();
        panel.selected = panel
            .rows
            .iter()
            .position(|r| r.label.starts_with(expected))
            .unwrap();
        let before_acceptance = docs.editor.buffer.revision;
        services.accept(&mut docs);
        let accept_started = Instant::now();
        while docs.editor.buffer.revision == before_acceptance {
            services.poll(&mut docs);
            assert!(
                accept_started.elapsed() < Duration::from_secs(10),
                "{id} completion acceptance failed: {:?}",
                services.message
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!docs.editor.buffer.text.to_string().contains("${"));
        eprintln!(
            "REAL EDITOR {id}: retained session through typing and accepted rebased completion"
        );
        if [
            "rust-analyzer",
            "clangd",
            "typescript-language-server",
            "pyright",
            "json-language-server",
        ]
        .contains(&id)
        {
            docs.editor.snippet = None;
            let content = docs.editor.buffer.text.to_string();
            docs.editor.buffer.cursor =
                content[..content.rfind(expected).unwrap()].chars().count() + 1;
            services.update(&docs);
            let started = Instant::now();
            while !services
                .diagnostics
                .iter()
                .any(|((_, document), (revision, _, _))| {
                    *document == docs.id() && *revision == docs.editor.buffer.revision
                })
            {
                services.poll(&mut docs);
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "{id}: no revision-current diagnostics received"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            eprintln!("REAL EDITOR {id}: received revision-current diagnostics");
            if services.runtime(id).iter().any(|r| {
                r.capabilities["definitionProvider"] == true
                    || r.capabilities["definitionProvider"].is_object()
            }) {
                services.request(&docs, Kind::Definition, None);
                let started = Instant::now();
                let mut requested = Instant::now();
                loop {
                    services.poll(&mut docs);
                    if requested.elapsed() > Duration::from_millis(500) {
                        services.request(&docs, Kind::Definition, None);
                        requested = Instant::now();
                    }
                    if services
                        .message
                        .as_ref()
                        .is_some_and(|m| m.contains("-32801"))
                    {
                        services.message = None;
                        services.request(&docs, Kind::Definition, None);
                    }
                    if services
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.kind == Kind::Definition && !p.rows.is_empty())
                    {
                        break;
                    }
                    assert!(
                        started.elapsed() < Duration::from_secs(20),
                        "{id}: no definition location: {:?}",
                        services.message
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                eprintln!("REAL EDITOR {id}: definition navigation returned a location");
            } else {
                eprintln!("REAL EDITOR {id}: definition unsupported");
            }
            let runtime = services.runtime(id);
            let caps = &runtime
                .iter()
                .find(|server| server.state == "Running")
                .unwrap()
                .capabilities;
            for capability in [
                "hoverProvider",
                "signatureHelpProvider",
                "referencesProvider",
                "documentSymbolProvider",
                "workspaceSymbolProvider",
                "documentFormattingProvider",
                "renameProvider",
                "codeActionProvider",
            ] {
                let advertised = !caps[capability].is_null() && caps[capability] != false;
                eprintln!(
                    "REAL CAPABILITY {id} {capability}: {}",
                    if advertised {
                        "advertised, behavior not exercised"
                    } else {
                        "unsupported"
                    }
                );
            }
            real_editor_navigation(&mut services, &mut docs, id, caps);
            services.control(id, false, false);
            services.update(&docs);
            assert!(services.runtime(id).is_empty());
            services.control(id, true, false);
            let restart_started = Instant::now();
            while !services
                .runtime(id)
                .iter()
                .any(|server| server.state == "Running")
            {
                services.poll(&mut docs);
                services.update(&docs);
                assert!(
                    restart_started.elapsed() < Duration::from_secs(20),
                    "{id}: did not restart"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            eprintln!("REAL EDITOR {id}: stopped and restarted");
        }
        if id == "basedpyright" {
            let start = Instant::now();
            while !services
                .diagnostics
                .iter()
                .any(|((server, _), (_, _, items))| {
                    server.starts_with("ruff@") && !items.is_empty()
                })
            {
                services.poll(&mut docs);
                services.update(&docs);
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "Ruff did not publish diagnostics: {:?}",
                    services.runtime("ruff")
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            eprintln!("REAL EDITOR Ruff: nonempty diagnostics alongside basedpyright");
        }
        services.shutdown();
        let stopped = Instant::now();
        while !services.stopped() {
            services.poll(&mut docs);
            assert!(stopped.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    #[ignore = "downloads and runs executable tools; requires explicit user consent"]
    fn confirmed_five_installers_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("tools");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let cancel = AtomicBool::new(false);
        let mut paths = BTreeSet::new();
        for file in ["test.rs", "test.ts", "test.py"] {
            let path = workspace.join(file);
            std::fs::write(&path, "").unwrap();
            paths.insert(path);
        }
        for entry in lsp_catalog::CATALOG {
            let started = Instant::now();
            let snapshot = execute(
                &data,
                &workspace,
                &paths,
                Operation::Install(entry.id.into()),
                &cancel,
                &|s| eprintln!("{}: {s}", entry.id),
            )
            .unwrap_or_else(|e| panic!("{} installation failed: {e:#}", entry.id));
            let installed = &snapshot.store.installed[entry.id];
            eprintln!(
                "VERIFIED {} version={} elapsed_ms={} command={} args={:?}",
                entry.id,
                installed.version,
                started.elapsed().as_millis(),
                installed.launch.command.display(),
                installed.launch.args
            );
            assert!(installed.launch.command.is_file());
            assert!(snapshot.statuses[entry.id].launch.is_some());
        }
        std::fs::write(workspace.join("package.json"), "{}").unwrap();
        std::fs::write(workspace.join("pyproject.toml"), "").unwrap();
        real_editor_completion(
            &data,
            &workspace,
            "vtsls",
            "test.ts",
            "const sample = { property: 1 };\nsample.pro",
            "property",
        );
        real_editor_completion(
            &data,
            &workspace,
            "typescript-native",
            "test.ts",
            "const sample = { property: 1 };\nsample.pro",
            "property",
        );
        real_editor_completion(
            &data,
            &workspace,
            "basedpyright",
            "test.py",
            "missing_name\ntext = \"hi\"\ntext.upp",
            "upper",
        );
        let checked = execute(
            &data,
            &workspace,
            &paths,
            Operation::CheckUpdates,
            &cancel,
            &|_| {},
        )
        .unwrap();
        assert!(checked.statuses.values().all(|s| s.available.is_some()));
        // Repeat one update to exercise generation publication and preservation.
        let old = load(&data).unwrap().installed["vtsls"].directory.clone();
        let updated = execute(
            &data,
            &workspace,
            &paths,
            Operation::Install("vtsls".into()),
            &cancel,
            &|s| eprintln!("update: {s}"),
        )
        .unwrap();
        assert_ne!(old, updated.store.installed["vtsls"].directory);
        assert!(old.is_dir());
        for entry in lsp_catalog::CATALOG {
            execute(
                &data,
                &workspace,
                &BTreeSet::new(),
                Operation::Remove(entry.id.into()),
                &cancel,
                &|_| {},
            )
            .unwrap();
            assert!(!load(&data).unwrap().installed.contains_key(entry.id));
        }
        assert!(
            std::fs::read_dir(data.join("packages/vtsls"))
                .unwrap()
                .next()
                .is_none()
        );
        eprintln!(
            "All five install/initialize/remove checks and a replacement update passed; temporary directory removed on return."
        );
    }
    #[test]
    fn recommendations_are_exclusive_and_python_is_complementary() {
        let mut prefs = Preferences::default();
        assert!(selected(
            lsp_catalog::entry("typescript-language-server").unwrap(),
            &prefs
        ));
        assert!(!selected(
            lsp_catalog::entry("typescript-native").unwrap(),
            &prefs
        ));
        assert!(selected(lsp_catalog::entry("pyright").unwrap(), &prefs));
        assert!(selected(lsp_catalog::entry("ruff").unwrap(), &prefs));
        for language in lsp_catalog::entry("typescript-native").unwrap().languages {
            prefs
                .primary
                .insert((*language).into(), "typescript-native".into());
        }
        assert!(!selected(
            lsp_catalog::entry("typescript-language-server").unwrap(),
            &prefs
        ));
        assert!(selected(
            lsp_catalog::entry("typescript-native").unwrap(),
            &prefs
        ));
        let mut mixed = Preferences::default();
        mixed
            .primary
            .insert("typescript".into(), "typescript-native".into());
        assert!(!selected_for(
            lsp_catalog::entry("typescript-language-server").unwrap(),
            "typescript",
            &mixed
        ));
        assert!(selected_for(
            lsp_catalog::entry("typescript-language-server").unwrap(),
            "javascript",
            &mixed
        ));
        assert!(selected_for(
            lsp_catalog::entry("typescript-native").unwrap(),
            "typescript",
            &mixed
        ));
        prefs.all_disabled = Some(true);
        assert!(
            lsp_catalog::CATALOG
                .iter()
                .all(|entry| !selected(entry, &prefs))
        );
    }
    #[test]
    fn workspace_overrides_and_dismissals_round_trip_without_touching_global_config() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let mut store = Store::default();
        store.global.disabled.insert("ruff".into());
        let local = store
            .workspaces
            .entry(crate::lsp::path_key(&workspace))
            .or_default();
        local.enabled.insert("ruff".into());
        local.dismissed.insert("basedpyright".into());
        local
            .primary
            .insert("typescript".into(), "typescript-native".into());
        save(temp.path(), &store).unwrap();
        let loaded = load(temp.path()).unwrap();
        assert!(loaded.global.disabled.contains("ruff"));
        assert!(!loaded.preferences(&workspace).disabled.contains("ruff"));
        assert!(
            loaded
                .preferences(&workspace)
                .dismissed
                .contains("basedpyright")
        );
        assert_eq!(
            loaded.preferences(&workspace).primary["typescript"],
            "typescript-native"
        );
    }
    #[test]
    fn invalid_state_is_preserved_and_not_silently_reset() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        std::fs::write(&path, "broken").unwrap();
        assert!(load(temp.path()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }
    #[test]
    fn receipt_ownership_rejects_external_and_wrong_tool_directories() {
        let temp = tempfile::tempdir().unwrap();
        let owned = temp.path().join("packages/ruff/generation-test");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::write(owned.join(".filavim-owned"), "ruff").unwrap();
        assert!(safe_directory(temp.path(), "ruff", &owned).is_ok());
        assert!(safe_directory(temp.path(), "basedpyright", &owned).is_err());
        assert!(safe_directory(temp.path(), "ruff", temp.path()).is_err());
        std::fs::write(owned.join(".filavim-owned"), "external").unwrap();
        assert!(safe_directory(temp.path(), "ruff", &owned).is_err());
    }
    #[test]
    fn nested_roots_and_project_node_launch_are_resolved_without_shells() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("packages/a");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(temp.path().join(".git"), "").unwrap();
        std::fs::write(project.join("package.json"), "{}").unwrap();
        let file = project.join("src/main.ts");
        assert_eq!(
            lsp_catalog::root(&file, lsp_catalog::entry("vtsls").unwrap()),
            project
        );
        let package = project.join("node_modules/@vtsls/language-server");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"version":"1.2.3","bin":{"vtsls":"server.js"}}"#,
        )
        .unwrap();
        std::fs::write(package.join("server.js"), "").unwrap();
        if crate::setup::executable("node").is_some() {
            let launch = npm_launch(&project, "@vtsls/language-server", "vtsls").unwrap();
            assert!(launch.args[0].ends_with("server.js"));
            std::fs::write(
                package.join("package.json"),
                r#"{"bin":{"vtsls":"../../../escape.js"}}"#,
            )
            .unwrap();
            std::fs::write(project.join("escape.js"), "").unwrap();
            assert!(npm_launch(&project, "@vtsls/language-server", "vtsls").is_err());
        }
    }
    #[test]
    fn cancellation_missing_dependencies_and_invalid_launch_fail_before_activation() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::default();
        save(temp.path(), &store).unwrap();
        let result = execute(
            temp.path(),
            temp.path(),
            &BTreeSet::new(),
            Operation::Refresh,
            &AtomicBool::new(true),
            &|_| {},
        );
        assert!(result.is_err());
        assert!(load(temp.path()).unwrap().installed.is_empty());
        assert!(tool("filavim-missing-runtime-fixture-419003").is_err());
        assert!(
            validate_launch(&Launch {
                command: temp.path().join("missing.exe"),
                args: vec![]
            })
            .is_err()
        );
        assert!(json_url("http://example.invalid/package", &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn failed_replacement_preserves_previous_installation_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::default();
        store.installed.insert(
            "ruff".into(),
            Installation {
                directory: temp.path().join("old"),
                version: "old".into(),
                launch: Launch {
                    command: "old".into(),
                    args: vec![],
                },
                source: "fixture".into(),
            },
        );
        save(temp.path(), &store).unwrap();
        let before = std::fs::read(temp.path().join("state.json")).unwrap();
        assert!(
            install(
                temp.path(),
                lsp_catalog::entry("ruff").unwrap(),
                &mut store,
                &AtomicBool::new(true),
                &|_| {}
            )
            .is_err()
        );
        assert_eq!(
            before,
            std::fs::read(temp.path().join("state.json")).unwrap()
        );
    }
    #[test]
    fn atomic_persistence_failure_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file"), "unchanged").unwrap();
        assert!(save(&temp.path().join("file"), &Store::default()).is_err());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("file")).unwrap(),
            "unchanged"
        );
    }
    fn http_fixture(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream.write_all(response).unwrap();
        });
        (origin, server)
    }
    #[test]
    fn downloader_streams_rejects_partial_http_errors_and_cross_origin_redirects() {
        for (response, successful) in [
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc"[..], true),
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 90\r\nConnection: close\r\n\r\nshort"[..], false),
            (&b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..], false),
            (&b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/never-requested\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..], false),
        ] {
            let (origin, server) = http_fixture(response);
            let destination = tempfile::NamedTempFile::new().unwrap();
            let result = download_from(&format!("{origin}/artifact"),destination.path(),&AtomicBool::new(false),1024,&|_|{},DownloadSource::Loopback(origin));
            assert_eq!(result.is_ok(), successful, "{result:?}");
            if successful { assert_eq!(std::fs::read(destination.path()).unwrap(), b"abc"); }
            server.join().unwrap();
        }
    }
    #[test]
    fn download_cancellation_terminates_a_pending_transfer_without_activation() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                .unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let cancellation = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancellation.clone();
        let destination = tempfile::NamedTempFile::new().unwrap();
        let path = destination.path().to_owned();
        let worker = std::thread::spawn(move || {
            download_from(
                &format!("{origin}/artifact"),
                &path,
                &worker_cancel,
                1024,
                &|_| {},
                DownloadSource::Loopback(origin),
            )
        });
        accepted_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        cancellation.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        release_tx.send(()).unwrap();
        server.join().unwrap();
    }
    #[test]
    #[ignore = "opt-in official downloads and real five-server completion acceptance"]
    fn required_catalog_install_and_completion_acceptance() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("tools");
        let workspace = temporary.path().join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname=\"acceptance\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[[bin]]\nname=\"acceptance\"\npath=\"main.rs\"\n").unwrap();
        std::fs::write(
            workspace.join("tsconfig.json"),
            "{\"compilerOptions\":{\"strict\":true}}",
        )
        .unwrap();
        std::fs::write(
            workspace.join("schema.json"),
            r#"{"type":"object","properties":{"property":{"type":"string","description":"Name shown in the fixture"}}}"#,
        )
        .unwrap();
        for (id, file, text, expected) in [
            (
                "rust-analyzer",
                "main.rs",
                "struct Widget { field: u32 }\nfn main() { let w = Widget { field: 1 }; w.fi",
                "field",
            ),
            (
                "clangd",
                "main.cpp",
                "struct Widget { int field; }; int main() { Widget w; w.fi",
                "field",
            ),
            (
                "typescript-language-server",
                "main.ts",
                "const widget = { property: 1 };\nwidget.pro",
                "property",
            ),
            (
                "pyright",
                "main.py",
                "class Thing:\n    def upper(self) -> str:\n        return 'hi'\ntext = Thing()\ntext.upp",
                "upper",
            ),
            (
                "json-language-server",
                "test.json",
                "{\"$schema\":\"./schema.json\",\"pro",
                "property",
            ),
        ] {
            let result = execute(
                &data,
                &workspace,
                &BTreeSet::new(),
                Operation::Install(id.into()),
                &AtomicBool::new(false),
                &|message| eprintln!("{id}: {message}"),
            );
            let snapshot = result.unwrap_or_else(|error| panic!("{id} install failed: {error:#}"));
            eprintln!(
                "REAL VERSION {id}: {}",
                snapshot.store.installed[id].version
            );
            real_editor_completion(&data, &workspace, id, file, text, expected);
        }
    }
}
