//! Local crash snapshots. One worker owns atomic writes and session locks.
use crate::{files::DiskState, gui::documents::Documents};
use anyhow::{Context, Result, bail};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
// JSON may escape one text byte into six bytes (e.g. NUL -> \u0000).
const MAX_ENCODED_BYTES: usize = MAX_BYTES * 6 + 64 * 1024;
#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub path: Option<PathBuf>,
    pub baseline: Option<DiskState>,
    pub text: String,
    pub cursor: usize,
    pub timestamp: u64,
    #[serde(skip)]
    pub source: PathBuf,
}
pub struct Snapshot {
    source: Option<PathBuf>,
    path: Option<PathBuf>,
    baseline: Option<DiskState>,
    text: Rope,
    cursor: usize,
}
pub enum Event {
    Available(Vec<Entry>),
    Error(String),
}
type Pending = Arc<Mutex<BTreeMap<u64, Option<Snapshot>>>>;
/// The UI only clones Rope roots and replaces pending state. The worker coalesces
/// updates for 750ms, with a two-second maximum delay during continuous typing.
pub struct Recovery {
    pending: Pending,
    signal: mpsc::SyncSender<()>,
    pub events: mpsc::Receiver<Event>,
    observed: BTreeMap<u64, (u64, bool, Option<PathBuf>, Option<DiskState>)>,
    sources: BTreeMap<u64, PathBuf>,
    discard: mpsc::Sender<Vec<PathBuf>>,
    flush: mpsc::Sender<mpsc::Sender<()>>,
    _worker: std::thread::JoinHandle<()>,
}
impl Recovery {
    pub fn new(root: PathBuf) -> Self {
        let pending: Pending = Arc::default();
        let jobs = pending.clone();
        let (signal, rx) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::channel();
        let (discard, deletes) = mpsc::channel::<Vec<PathBuf>>();
        let (flush, flushes) = mpsc::channel::<mpsc::Sender<()>>();
        let worker = std::thread::spawn(move || {
            let run = || -> Result<()> {
                fs::create_dir_all(&root)?;
                let session = tempfile::Builder::new()
                    .prefix("session-")
                    .tempdir_in(&root)?
                    .keep();
                let lock = lock_file(session.join("lock"))?;
                lock.try_lock().context("Cannot lock recovery session")?;
                let mut locks = Vec::new();
                let mut available = Vec::new();
                for directory in fs::read_dir(&root)? {
                    let directory = match directory {
                        Ok(entry) => entry.path(),
                        Err(error) => {
                            report_error(
                                &tx,
                                format!("Cannot enumerate recovery sessions: {error}"),
                            );
                            continue;
                        }
                    };
                    if directory == session {
                        continue;
                    }
                    // Do not follow a directory symlink outside local recovery storage.
                    match fs::symlink_metadata(&directory) {
                        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                        }
                        Ok(_) => continue,
                        Err(error) => {
                            report_error(
                                &tx,
                                format!(
                                    "Cannot inspect recovery session {}: {error}",
                                    directory.display()
                                ),
                            );
                            continue;
                        }
                    }
                    let old_lock = match lock_file(directory.join("lock")) {
                        Ok(lock) => lock,
                        Err(error) => {
                            report_error(
                                &tx,
                                format!(
                                    "Cannot inspect recovery session {}: {error}",
                                    directory.display()
                                ),
                            );
                            continue;
                        }
                    };
                    match old_lock.try_lock() {
                        Ok(()) => {}
                        Err(std::fs::TryLockError::WouldBlock) => continue,
                        Err(error) => {
                            report_error(
                                &tx,
                                format!(
                                    "Cannot lock recovery session {}: {error}",
                                    directory.display()
                                ),
                            );
                            continue;
                        }
                    }
                    scan_session(&directory, &tx, &mut available);
                    if fs::read_dir(&directory).is_ok_and(|mut entries| {
                        entries.all(|entry| entry.is_ok_and(|e| e.file_name() == "lock"))
                    }) {
                        drop(old_lock);
                        let _ = fs::remove_file(directory.join("lock"));
                        let _ = fs::remove_dir(&directory);
                    } else {
                        locks.push(old_lock);
                    }
                }
                if !available.is_empty() {
                    let _ = tx.send(Event::Available(available));
                    crate::background::wake();
                }
                while rx.recv().is_ok() {
                    let mut acknowledgments: Vec<_> = flushes.try_iter().collect();
                    let started = Instant::now();
                    while acknowledgments.is_empty() && started.elapsed() < Duration::from_secs(2) {
                        match rx.recv_timeout(snapshot_wait(started.elapsed())) {
                            Ok(()) => {
                                acknowledgments.extend(flushes.try_iter());
                            }
                            Err(_) => break,
                        }
                    }
                    let batch = std::mem::take(&mut *jobs.lock().unwrap());
                    for (id, snapshot) in batch {
                        let path = session.join(format!("{id}.json"));
                        let label = snapshot.as_ref().and_then(|s| s.path.as_ref()).map_or_else(
                            || format!("Untitled ({id})"),
                            |p| p.display().to_string(),
                        );
                        let result = if let Some(snapshot) = snapshot {
                            write(&path, snapshot)
                        } else {
                            remove(&path)
                        };
                        if let Err(error) = result {
                            report_error(
                                &tx,
                                format!(
                                    "Recovery snapshot failed for {label}: {error:#}. Latest edits may not be protected; save a copy."
                                ),
                            );
                        }
                    }
                    for paths in deletes.try_iter() {
                        for path in paths {
                            if let Err(error) = remove(&path) {
                                let _ = tx.send(Event::Error(format!(
                                    "Cannot discard recovery entry: {error:#}"
                                )));
                            }
                        }
                    }
                    for ack in acknowledgments {
                        let _ = ack.send(());
                    }
                    crate::background::wake();
                }
                drop(locks);
                drop(lock);
                let _ = fs::remove_file(session.join("lock"));
                let _ = fs::remove_dir(&session);
                Ok(())
            };
            if let Err(error) = run() {
                let _ = tx.send(Event::Error(format!("Recovery unavailable: {error:#}")));
                crate::background::wake();
            }
        });
        Self {
            pending,
            signal,
            events,
            observed: BTreeMap::new(),
            sources: BTreeMap::new(),
            discard,
            flush,
            _worker: worker,
        }
    }
    pub fn flush(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        let _ = self.flush.send(tx);
        let _ = self.signal.try_send(());
        rx
    }
    pub fn adopt(&mut self, id: u64, source: PathBuf) {
        self.sources.insert(id, source);
    }
    pub fn sync(&mut self, documents: &Documents) {
        let mut pending = self.pending.lock().unwrap();
        for id in &documents.order {
            let Some(editor) = documents.get(*id) else {
                continue;
            };
            let buffer = &editor.buffer;
            let state = (
                buffer.state(),
                buffer.dirty(),
                buffer.path.clone(),
                buffer.disk_state.clone(),
            );
            if self.observed.get(id) == Some(&state) {
                continue;
            }
            self.observed.insert(*id, state);
            pending.insert(
                *id,
                buffer.dirty().then(|| Snapshot {
                    source: self.sources.get(id).cloned(),
                    path: buffer.path.clone(),
                    baseline: buffer.disk_state.clone(),
                    text: buffer.text.clone(),
                    cursor: buffer.cursor,
                }),
            );
        }
        self.observed.retain(|id, _| {
            if documents.get(*id).is_none() {
                pending.insert(*id, None);
                false
            } else {
                true
            }
        });
        self.sources
            .retain(|id, _| documents.get(*id).is_some_and(|e| e.buffer.dirty()));
        if !pending.is_empty() {
            let _ = self.signal.try_send(());
        }
    }
    pub fn discard(&self, entries: &[Entry]) {
        self.discard_paths(entries.iter().map(|e| e.source.clone()).collect());
    }
    pub fn discard_paths(&self, paths: Vec<PathBuf>) {
        let _ = self.discard.send(paths);
        let _ = self.signal.try_send(());
    }
}
fn snapshot_wait(elapsed: Duration) -> Duration {
    Duration::from_millis(750).min(Duration::from_secs(2).saturating_sub(elapsed))
}
fn report_error(tx: &mpsc::Sender<Event>, message: String) {
    let _ = tx.send(Event::Error(message));
    crate::background::wake();
}
fn scan_session(directory: &std::path::Path, tx: &mpsc::Sender<Event>, available: &mut Vec<Entry>) {
    let files = match fs::read_dir(directory) {
        Ok(files) => files,
        Err(error) => {
            report_error(
                tx,
                format!(
                    "Cannot read recovery session {}: {error}",
                    directory.display()
                ),
            );
            return;
        }
    };
    for file in files {
        let file = match file {
            Ok(file) => file,
            Err(error) => {
                report_error(
                    tx,
                    format!(
                        "Cannot enumerate recovery entry in {}: {error}",
                        directory.display()
                    ),
                );
                continue;
            }
        };
        let path = file.path();
        match file.file_type() {
            Ok(kind) if kind.is_file() => {}
            Ok(_) => continue,
            Err(error) => {
                report_error(
                    tx,
                    format!("Cannot inspect recovery entry {}: {error}", path.display()),
                );
                continue;
            }
        }
        let temporary = file.file_name().to_string_lossy().starts_with(".tmp");
        if path.extension().is_none_or(|ext| ext != "json") && !temporary {
            continue;
        }
        let result = if temporary {
            Err(anyhow::anyhow!("Interrupted temporary snapshot"))
        } else {
            load(&path)
        };
        match result {
            Ok(entry) => available.push(entry),
            Err(error) => {
                let quarantine = quarantine(&path);
                let location = match quarantine {
                    Ok(path) => format!("Preserved at {}", path.display()),
                    Err(move_error) => {
                        format!("Original retained; quarantine failed: {move_error}")
                    }
                };
                report_error(
                    tx,
                    format!("Cannot restore {}: {error}. {location}", path.display()),
                );
            }
        }
    }
}
fn quarantine(path: &std::path::Path) -> Result<PathBuf> {
    let temp = tempfile::Builder::new()
        .prefix("damaged-")
        .suffix(".quarantine")
        .tempfile_in(path.parent().context("Recovery entry has no parent")?)?;
    let (file, destination) = temp.keep()?;
    drop(file);
    if let Err(error) = fs::rename(path, &destination) {
        let _ = fs::remove_file(&destination); // Only our reserved empty file.
        return Err(error.into());
    }
    Ok(destination)
}
fn lock_file(path: PathBuf) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}
fn remove(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn load(path: &std::path::Path) -> Result<Entry> {
    if fs::metadata(path)?.len() > MAX_ENCODED_BYTES as u64 {
        bail!("Recovery snapshot exceeds size limit");
    }
    let reader = BufReader::new(File::open(path)?).take(MAX_ENCODED_BYTES as u64 + 1);
    let mut entry: Entry = serde_json::from_reader(reader).map_err(|error| {
        // Serde's Display may include a malformed field's value. Never echo it.
        anyhow::anyhow!(
            "Invalid recovery JSON ({:?}) at line {}, column {}",
            error.classify(),
            error.line(),
            error.column()
        )
    })?;
    if entry.text.len() > MAX_BYTES {
        bail!("Recovery document exceeds size limit");
    }
    entry.source = path.into();
    Ok(entry)
}
fn write(path: &std::path::Path, snapshot: Snapshot) -> Result<()> {
    if snapshot.text.len_bytes() > MAX_BYTES {
        bail!("Document exceeds the 16 MiB recovery limit");
    }
    let source = snapshot.source;
    let entry = Entry {
        path: snapshot.path,
        baseline: snapshot.baseline,
        text: snapshot.text.to_string(),
        cursor: snapshot.cursor,
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        source: PathBuf::new(),
    };
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, &entry)?;
        writer.flush()?;
    }
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    if let Some(source) = source {
        remove(&source)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Buffer, config::Config, gui::model::Model};
    fn documents(text: &str, path: Option<PathBuf>) -> Documents {
        let mut config = Config::default();
        config.lsp.enabled = false;
        let mut model = Model::new(config).unwrap();
        let mut buffer = Buffer::new(Rope::new(), path);
        buffer.insert(text);
        model.set_buffer(buffer);
        Documents::new(model)
    }
    fn stop(recovery: Recovery) {
        let Recovery {
            signal,
            discard,
            flush,
            _worker,
            ..
        } = recovery;
        drop(signal);
        drop(discard);
        drop(flush);
        _worker.join().unwrap();
    }
    #[test]
    fn untitled_and_file_documents_survive_worker_restart_and_saved_entries_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = Recovery::new(dir.path().into());
        let mut recovery = recovery;
        let mut docs = documents("untitled 日本", None);
        let mut second = Buffer::new(Rope::new(), Some(dir.path().join("original.rs")));
        second.disk_state = Some(DiskState::Missing);
        second.insert("file contents");
        docs.add(second, false).unwrap();
        recovery.sync(&docs);
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        stop(recovery);
        let recovery = Recovery::new(dir.path().into());
        let Event::Available(entries) = recovery
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected recovery entries")
        };
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|e| e.text == "untitled 日本" && e.path.is_none())
        );
        assert!(
            entries
                .iter()
                .any(|e| e.text == "file contents" && e.baseline == Some(DiskState::Missing))
        );
        recovery.discard(&entries);
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(entries.iter().all(|e| !e.source.exists()));
        stop(recovery);
    }
    #[test]
    fn active_sessions_are_not_offered_and_clean_buffers_remove_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let mut recovery = Recovery::new(dir.path().into());
        let mut docs = documents("unsaved", None);
        recovery.sync(&docs);
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let other = Recovery::new(dir.path().into());
        other.flush().recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(other.events.try_recv().is_err());
        let state = docs.editor.buffer.state();
        docs.editor.buffer.mark_saved(state);
        recovery.sync(&docs);
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        stop(other);
        stop(recovery);
        let restarted = Recovery::new(dir.path().into());
        restarted
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(restarted.events.try_recv().is_err());
        stop(restarted);
    }
    #[test]
    fn interrupted_temporary_write_does_not_replace_committed_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        write(
            &path,
            Snapshot {
                source: None,
                path: None,
                baseline: None,
                text: Rope::from_str("committed"),
                cursor: 3,
            },
        )
        .unwrap();
        let mut interrupted = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        interrupted.write_all(b"{partial").unwrap();
        assert_eq!(load(&path).unwrap().text, "committed");
        assert_eq!(load(&path).unwrap().cursor, 3);
    }
    #[test]
    fn recovery_process_fixture() {
        let Some(root) = std::env::var_os("FILAVIM_RECOVERY_CHILD") else {
            return;
        };
        let mut recovery = Recovery::new(root.into());
        let docs = documents("process recovery", None);
        recovery.sync(&docs);
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        println!("READY");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    #[test]
    fn unexpected_process_termination_releases_session_and_preserves_snapshot() {
        use std::io::{BufRead, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recovery::tests::recovery_process_fixture",
                "--nocapture",
            ])
            .env("FILAVIM_RECOVERY_CHILD", dir.path())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if line.trim() == "READY" {
                    let _ = tx.send(());
                    break;
                }
            }
        });
        let ready = rx.recv_timeout(Duration::from_secs(10));
        let _ = child.kill();
        let _ = child.wait();
        ready.unwrap();
        let recovery = Recovery::new(dir.path().into());
        let Event::Available(entries) = recovery
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            panic!("missing process recovery")
        };
        assert_eq!(entries[0].text, "process recovery");
        stop(recovery);
    }
    fn snapshot(text: &str) -> Snapshot {
        Snapshot {
            source: None,
            path: None,
            baseline: None,
            text: Rope::from_str(text),
            cursor: 0,
        }
    }
    #[test]
    fn mixed_valid_damaged_and_interrupted_entries_are_independent_and_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("session-old");
        fs::create_dir(&old).unwrap();
        write(&old.join("valid.json"), snapshot("recover valid")).unwrap();
        let damaged =
            br#"{"path":"SECRET_CONTENT_NOT_FOR_LOGS","baseline":"SECRET_CONTENT_NOT_FOR_LOGS"}"#;
        fs::write(old.join("corrupt.json"), damaged).unwrap();
        fs::write(old.join(".tmp-interrupted"), b"partial data").unwrap();
        let recovery = Recovery::new(dir.path().into());
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let mut recovered = 0;
        let mut errors = 0;
        for event in recovery.events.try_iter() {
            match event {
                Event::Available(entries) => {
                    assert_eq!(entries[0].text, "recover valid");
                    recovered += entries.len();
                }
                Event::Error(message) => {
                    assert!(!message.contains("SECRET_CONTENT_NOT_FOR_LOGS"));
                    assert!(message.contains("Preserved at"));
                    errors += 1;
                }
            }
        }
        assert_eq!((recovered, errors), (1, 2));
        let files: Vec<_> = fs::read_dir(&old)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let quarantine: Vec<_> = files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "quarantine"))
            .collect();
        assert_eq!(quarantine.len(), 2);
        assert!(quarantine.iter().any(|p| fs::read(p).unwrap() == damaged));
        assert!(!old.join("corrupt.json").exists());
        stop(recovery);
    }
    // The encoded-limit regression intentionally writes >32 MiB; Full only.
    #[cfg(feature = "test-tools")]
    #[test]
    fn escaped_text_under_limit_is_readable_and_oversize_write_keeps_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        // Former encoded-size check rejected this valid 6 MiB document (>32 MiB JSON).
        let text = "\0".repeat(6 * 1024 * 1024);
        write(&path, snapshot(&text)).unwrap();
        assert_eq!(load(&path).unwrap().text, text);
        let oversized = "x".repeat(MAX_BYTES + 1);
        assert!(
            write(&path, snapshot(&oversized))
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
        assert_eq!(load(&path).unwrap().text, text);
    }
    #[test]
    fn unavailable_root_reports_failure_and_flush_does_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-a-directory");
        fs::write(&root, "occupied").unwrap();
        let recovery = Recovery::new(root);
        assert!(matches!(
            recovery
                .events
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            Event::Error(_)
        ));
        assert!(
            recovery
                .flush()
                .recv_timeout(Duration::from_secs(5))
                .is_err()
        );
        stop(recovery);
    }
    #[test]
    #[ignore = "opt-in recovery escaped-text I/O measurement"]
    fn escaped_snapshot_io_measurement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        let text = "\0".repeat(16 * 1024);
        let start = Instant::now();
        for _ in 0..3 {
            write(&path, snapshot(&text)).unwrap();
            assert_eq!(load(&path).unwrap().text.len(), text.len());
        }
        println!(
            "FILAVIM_METRICS {}",
            serde_json::json!({"workload":"escaped_snapshot_write_and_load", "text_bytes":text.len(), "samples":3, "total_ns":start.elapsed().as_nanos() as u64, "optimized":!cfg!(debug_assertions)})
        );
    }
    #[test]
    fn debounce_never_waits_past_the_maximum_snapshot_age() {
        assert_eq!(snapshot_wait(Duration::ZERO), Duration::from_millis(750));
        assert_eq!(
            snapshot_wait(Duration::from_millis(1900)),
            Duration::from_millis(100)
        );
        assert_eq!(snapshot_wait(Duration::from_secs(3)), Duration::ZERO);
    }
    #[cfg(windows)]
    #[test]
    fn quarantine_failure_retains_damaged_data_and_does_not_block_valid_entries() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("session-old");
        fs::create_dir(&old).unwrap();
        let damaged = old.join("locked.json");
        fs::write(&damaged, b"{bad").unwrap();
        write(&old.join("valid.json"), snapshot("valid")).unwrap();
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&damaged)
            .unwrap();
        let recovery = Recovery::new(dir.path().into());
        recovery
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let mut errors = 0;
        let mut restored = 0;
        for event in recovery.events.try_iter() {
            match event {
                Event::Error(message) => {
                    assert!(message.contains("Original retained"));
                    errors += 1;
                }
                Event::Available(entries) => {
                    assert_eq!(entries[0].text, "valid");
                    restored += entries.len();
                }
            }
        }
        assert_eq!((errors, restored), (1, 1));
        assert_eq!(fs::read(&damaged).unwrap(), b"{bad");
        stop(recovery);
        drop(handle);
    }
}
