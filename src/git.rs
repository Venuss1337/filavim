//! Git jobs are serialized on a worker; no repository subprocess runs on input.
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub debounce_ms: u64,
    pub refresh_ms: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 400,
            refresh_ms: 5000,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub root: PathBuf,
    pub branch: String,
    pub files: BTreeMap<PathBuf, String>,
    pub normalized_files: BTreeMap<String, String>,
    pub lines: BTreeMap<usize, char>,
    pub lines_unavailable: Option<String>,
    pub path: Option<PathBuf>,
    pub hunks: Vec<Hunk>,
    pub staged: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub title: String,
    pub patch: String,
    pub line: usize,
}
pub enum Job {
    Refresh {
        root: PathBuf,
        path: Option<PathBuf>,
        text: Option<ropey::Rope>,
        revision: u64,
    },
    Diff {
        root: PathBuf,
        path: PathBuf,
        staged: bool,
    },
    Stage {
        root: PathBuf,
        path: PathBuf,
        undo: bool,
    },
    Hunk {
        root: PathBuf,
        path: PathBuf,
        patch: String,
        undo: bool,
    },
    Previous {
        root: PathBuf,
        path: PathBuf,
    },
    Commit {
        root: PathBuf,
        message: String,
    },
}
pub enum Event {
    State { state: State, revision: u64 },
    Diff(State),
    Previous { path: PathBuf, text: String },
    Message(String),
}
pub struct Service {
    pub sender: mpsc::Sender<Job>,
    pub events: mpsc::Receiver<Result<Event, String>>,
}
impl Default for Service {
    fn default() -> Self {
        let (sender, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            let mut cache = DiffCache::default();
            while let Ok(job) = rx.recv() {
                let result = match job {
                    Job::Refresh {
                        root,
                        path,
                        text,
                        revision,
                    } => inspect_cached(
                        &root,
                        path.as_deref(),
                        text.as_ref().map(|t| t.to_string()).as_deref(),
                        Some(&mut cache),
                    )
                    .map(|state| Event::State { state, revision }),
                    other => execute(other),
                };
                if tx.send(result.map_err(|e| format!("{e:#}"))).is_err() {
                    break;
                }
                crate::background::wake();
            }
        });
        Self { sender, events }
    }
}
/// The checked-out branch for the repository containing `start`, read from
/// `HEAD` without spawning Git (cheap enough for a status line, still blocking
/// file I/O: call it off the UI thread). Linked worktrees (`.git` files) are
/// followed; a detached `HEAD` yields its short commit id.
pub fn head_branch(start: &Path) -> Option<String> {
    let mut dir = Some(start);
    while let Some(at) = dir {
        let dot = at.join(".git");
        let git_dir = if dot.is_dir() {
            Some(dot)
        } else if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let target = Path::new(target);
            Some(if target.is_absolute() {
                target.to_path_buf()
            } else {
                at.join(target)
            })
        } else {
            None
        };
        if let Some(git_dir) = git_dir {
            let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
            let head = head.trim();
            return Some(match head.strip_prefix("ref:") {
                Some(name) => {
                    let name = name.trim();
                    name.strip_prefix("refs/heads/").unwrap_or(name).to_owned()
                }
                None => head.chars().take(7).collect(),
            });
        }
        dir = at.parent();
    }
    None
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let executable = crate::setup::executable("git")
        .context("Git is missing. Install Git and add it to PATH.")?;
    let mut c = Command::new(executable);
    c.current_dir(root)
        .args(["-c", "core.quotepath=false"])
        .args(args);
    crate::setup::run_data(
        c,
        Duration::from_secs(20),
        if args.first() == Some(&"status") {
            32 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        },
    )
}
fn relative(root: &Path, path: &Path) -> Result<String> {
    let root = crate::paths::display_path(root);
    let path = crate::paths::display_path(path);
    Path::new(&path)
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|_| anyhow::anyhow!("File is outside Git root"))
}
#[derive(Default)]
struct DiffCache {
    before: String,
    after: String,
    lines: BTreeMap<usize, char>,
}
pub fn inspect(root: &Path, path: Option<&Path>, text: Option<&str>) -> Result<State> {
    inspect_cached(root, path, text, None)
}
fn inspect_cached(
    root: &Path,
    path: Option<&Path>,
    text: Option<&str>,
    cache: Option<&mut DiffCache>,
) -> Result<State> {
    let repo = git(root, &["rev-parse", "--show-toplevel"])?;
    let repo = PathBuf::from(repo.trim());
    let output = git(&repo, &["status", "--porcelain=v1", "-z", "--branch"])?;
    let mut state = State {
        root: repo.clone(),
        path: path.map(Path::to_path_buf),
        ..State::default()
    };
    let mut entries = output.split('\0');
    while let Some(entry) = entries.next() {
        if let Some(branch) = entry.strip_prefix("## ") {
            state.branch = branch.split("...").next().unwrap_or(branch).into();
        } else if entry.len() > 3 {
            let status = entry[..2].to_owned();
            state.files.insert(repo.join(&entry[3..]), status.clone());
            if status.contains('R') || status.contains('C') {
                entries.next();
            }
        }
    }
    state.normalized_files = state
        .files
        .iter()
        .map(|(p, s)| (crate::lsp::path_key(p), s.clone()))
        .collect();
    if let Some(path) = path {
        let relative = relative(&repo, path)?;
        let baseline = if state.files.get(path).is_some_and(|s| s == "??") {
            String::new()
        } else {
            match git(&repo, &["show", &format!(":{relative}")]) {
                Ok(text) => text,
                Err(error) => {
                    state.lines_unavailable =
                        Some(format!("Git line indicators unavailable: {error:#}"));
                    return Ok(state);
                }
            }
        };
        let current = match text {
            Some(t) => std::borrow::Cow::Borrowed(t),
            None => std::borrow::Cow::Owned(std::fs::read_to_string(path).unwrap_or_default()),
        };
        if current.len() > 8 * 1024 * 1024 {
            state.lines_unavailable =
                Some("Git line indicators unavailable: document exceeds 8 MiB".into());
            return Ok(state);
        }
        state.lines = if let Some(cache) = cache {
            if cache.before == baseline && cache.after == current {
                cache.lines.clone()
            } else {
                let lines = line_changes(&baseline, &current);
                if baseline.len() + current.len() <= 8 * 1024 * 1024 {
                    cache.before = baseline;
                    cache.after = current.into_owned();
                    cache.lines = lines.clone();
                } else {
                    *cache = DiffCache::default();
                }
                lines
            }
        } else {
            line_changes(&baseline, &current)
        };
    }
    Ok(state)
}
pub fn line_changes(before: &str, after: &str) -> BTreeMap<usize, char> {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut lines = BTreeMap::new();
    let mut row = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => row += 1,
            similar::ChangeTag::Insert => {
                let mark = if lines.get(&row) == Some(&'-') {
                    '~'
                } else {
                    '+'
                };
                lines.insert(row, mark);
                row += 1;
            }
            similar::ChangeTag::Delete => {
                lines.insert(row, '-');
            }
        }
    }
    lines
}
pub fn split_hunks(patch: &str) -> Vec<Hunk> {
    let mut header = String::new();
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            let location = line
                .split_whitespace()
                .nth(2)
                .unwrap_or("+1")
                .trim_start_matches('+')
                .split(',')
                .next()
                .unwrap_or("1")
                .parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1);
            hunks.push(Hunk {
                title: line.trim().into(),
                patch: header.clone() + line,
                line: location,
            });
        } else if let Some(h) = hunks.last_mut() {
            h.patch.push_str(line)
        } else {
            header.push_str(line)
        }
    }
    hunks
}
pub fn apply_hunk(root: &Path, patch: &str, undo: bool) -> Result<()> {
    for check in [true, false] {
        let mut command = Command::new(crate::setup::executable("git").context("Git missing")?);
        command
            .current_dir(root)
            .args(["apply", "--cached", "--whitespace=nowarn"]);
        if undo {
            command.arg("--reverse");
        }
        if check {
            command.arg("--check");
        }
        crate::setup::hide_window(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(patch.as_bytes())?;
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Git apply timed out")
            };
            thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "Cannot apply hunk: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    Ok(())
}
pub fn execute(job: Job) -> Result<Event> {
    match job {
        Job::Refresh {
            root,
            path,
            text,
            revision,
        } => Ok(Event::State {
            state: inspect(
                &root,
                path.as_deref(),
                text.as_ref().map(|t| t.to_string()).as_deref(),
            )?,
            revision,
        }),
        Job::Diff { root, path, staged } => {
            let mut state = inspect(&root, Some(&path), None)?;
            let relative = relative(&state.root, &path)?;
            let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--unified=3"];
            if staged {
                args.push("--cached")
            }
            args.extend(["--", &relative]);
            let patch = git(&state.root, &args)?;
            state.hunks = split_hunks(&patch);
            state.staged = staged;
            Ok(Event::Diff(state))
        }
        Job::Stage { root, path, undo } => {
            let state = inspect(&root, None, None)?;
            let relative = relative(&state.root, &path)?;
            if undo {
                if git(&state.root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
                    git(&state.root, &["restore", "--staged", "--", &relative])?;
                } else {
                    git(&state.root, &["rm", "--cached", "--", &relative])?;
                }
            } else {
                git(&state.root, &["add", "--", &relative])?;
            }
            Ok(Event::Message(
                if undo { "File unstaged" } else { "File staged" }.into(),
            ))
        }
        Job::Hunk {
            root,
            path,
            patch,
            undo,
        } => {
            let state = inspect(&root, None, None)?;
            let relative = relative(&state.root, &path)?;
            let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--unified=3"];
            if undo {
                args.push("--cached")
            }
            args.extend(["--", &relative]);
            let current = git(&state.root, &args)?;
            if !split_hunks(&current).iter().any(|h| h.patch == patch) {
                bail!("Hunk changed; refresh the diff before staging")
            };
            apply_hunk(&state.root, &patch, undo)?;
            Ok(Event::Message("Hunk updated in index".into()))
        }
        Job::Previous { root, path } => {
            let state = inspect(&root, None, None)?;
            let relative = relative(&state.root, &path)?;
            let text = git(&state.root, &["show", &format!("HEAD:{relative}")])?;
            Ok(Event::Previous { path, text })
        }
        Job::Commit { root, message } => {
            if message.trim().is_empty() {
                bail!("Commit message is empty")
            };
            let output = git(&root, &["commit", "-m", &message])?;
            Ok(Event::Message(output))
        }
    }
}

#[cfg(test)]
mod head_tests {
    use super::head_branch;
    #[test]
    fn head_branch_reads_branches_worktrees_and_detached_heads() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feat/glass\n").unwrap();
        assert_eq!(
            head_branch(&repo.join("src/deep")).as_deref(),
            Some("feat/glass")
        );
        let worktree = temp.path().join("tree");
        std::fs::create_dir_all(temp.path().join("meta")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../meta\n").unwrap();
        std::fs::write(temp.path().join("meta/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(head_branch(&worktree).as_deref(), Some("0123456"));
    }
}
