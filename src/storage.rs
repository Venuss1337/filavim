//! Serialized file I/O and persistent MRU history; never executed by rendering/input.
use crate::{
    files,
    workspace::{BufferId, ViewId},
};
use anyhow::{Context, Result, bail};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct History {
    pub files: Vec<PathBuf>,
    pub projects: Vec<PathBuf>,
}
impl History {
    pub fn path() -> PathBuf {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        };
        base.unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("filavim/history.toml")
    }
    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn file(&mut self, path: PathBuf) {
        mru(&mut self.files, path);
    }
    pub fn project(&mut self, path: PathBuf) {
        mru(&mut self.projects, path);
    }
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        self.save_to(&path)
    }
    pub fn save_to(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path.parent().unwrap())?;
        files::save(path, &Rope::from_str(&toml::to_string(self)?))
    }
}
fn mru(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(100);
}

#[derive(Clone, Debug)]
pub enum Mutation {
    Create { path: PathBuf, directory: bool },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
}
fn checked(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let parent = path.parent().context("Path has no parent")?;
    let resolved =
        fs::canonicalize(parent)?.join(path.file_name().context("Path has no file name")?);
    if !resolved.starts_with(&root) || resolved == root {
        bail!("File operation must stay inside the workspace and cannot target its root");
    }
    Ok(resolved)
}
pub fn mutate(root: &Path, operation: &Mutation) -> Result<()> {
    match operation {
        Mutation::Create { path, directory } => {
            let path = checked(root, path)?;
            if *directory {
                fs::create_dir(&path)?;
            } else {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)?;
            }
        }
        Mutation::Rename { from, to } => {
            let from = checked(root, from)?;
            let to = checked(root, to)?;
            if fs::symlink_metadata(&to).is_ok() {
                bail!("Destination already exists");
            }
            fs::rename(from, to)?;
        }
        Mutation::Delete { path } => {
            let path = checked(root, path)?;
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                if cfg!(windows) && path.is_dir() {
                    fs::remove_dir(path)?;
                } else {
                    fs::remove_file(path)?;
                }
            } else if meta.is_dir() {
                // Resolve and validate the exact recursive target immediately before removal.
                let canonical = fs::canonicalize(&path)?;
                let root = fs::canonicalize(root)?;
                if canonical == root || !canonical.starts_with(&root) {
                    bail!("Refusing a directory deletion outside the workspace");
                }
                fs::remove_dir_all(canonical)?;
            } else {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}
pub enum IoJob {
    Open {
        serial: u64,
        view: ViewId,
        path: PathBuf,
        line: usize,
        column: usize,
        reload: Option<(BufferId, u64)>,
    },
    Save {
        buffer: BufferId,
        path: PathBuf,
        text: Rope,
        state: u64,
        quit: bool,
        protected: Vec<PathBuf>,
    },
    Search {
        buffer: BufferId,
        revision: u64,
        text: Rope,
        query: String,
        start: usize,
    },
    Root(PathBuf),
    Mutation {
        root: PathBuf,
        operation: Mutation,
    },
    History {
        history: History,
        path: PathBuf,
    },
}
pub enum IoEvent {
    Open {
        serial: u64,
        view: ViewId,
        path: PathBuf,
        line: usize,
        column: usize,
        reload: Option<(BufferId, u64)>,
        result: Result<Rope>,
    },
    Save {
        buffer: BufferId,
        path: PathBuf,
        state: u64,
        quit: bool,
        result: Result<()>,
        created: bool,
    },
    Search {
        buffer: BufferId,
        revision: u64,
        result: Option<usize>,
    },
    Root(Result<PathBuf>),
    Mutation {
        operation: Mutation,
        result: Result<()>,
    },
    History(Result<()>),
    Superseded,
}
pub struct IoWorker {
    pub sender: mpsc::Sender<IoJob>,
    pub results: mpsc::Receiver<IoEvent>,
}
impl Default for IoWorker {
    fn default() -> Self {
        let (sender, jobs) = mpsc::channel();
        let (tx, results) = mpsc::channel();
        thread::spawn(move || {
            let mut batch = std::collections::VecDeque::new();
            loop {
                if batch.is_empty() {
                    let Ok(first) = jobs.recv() else {
                        break;
                    };
                    batch.push_back(first);
                    batch.extend(jobs.try_iter().take(63));
                    coalesce_reads(&mut batch, &tx);
                }
                let job = batch.pop_front().unwrap();
                let result = match job {
                    IoJob::Open {
                        serial,
                        view,
                        path,
                        line,
                        column,
                        reload,
                    } => {
                        let path = fs::canonicalize(&path).unwrap_or(path);
                        let result = files::open(&path);
                        IoEvent::Open {
                            serial,
                            view,
                            path,
                            line,
                            column,
                            reload,
                            result,
                        }
                    }
                    IoJob::Save {
                        buffer,
                        path,
                        text,
                        state,
                        quit,
                        protected,
                    } => {
                        let created = !path.exists();
                        IoEvent::Save {
                            buffer,
                            result: (|| {
                                let destination =
                                    fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                                if protected.iter().any(|p| {
                                    fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == destination
                                }) {
                                    bail!(
                                        "Destination belongs to another open buffer or pending save"
                                    );
                                }
                                files::save(&path, &text)
                            })(),
                            path,
                            state,
                            quit,
                            created,
                        }
                    }
                    IoJob::Search {
                        buffer,
                        revision,
                        text,
                        query,
                        start,
                    } => IoEvent::Search {
                        buffer,
                        revision,
                        result: files::find(&text, &query, start),
                    },
                    IoJob::Root(path) => IoEvent::Root(
                        fs::canonicalize(path)
                            .context("Cannot change workspace")
                            .and_then(|p| {
                                if p.is_dir() {
                                    Ok(p)
                                } else {
                                    bail!("Workspace must be a directory")
                                }
                            }),
                    ),
                    IoJob::Mutation { root, operation } => IoEvent::Mutation {
                        result: mutate(&root, &operation),
                        operation,
                    },
                    IoJob::History { history, path } => IoEvent::History(history.save_to(&path)),
                };
                if tx.send(result).is_err() {
                    break;
                }
                crate::background::wake();
            }
        });
        Self { sender, results }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Replaceable {
    Open(ViewId),
    Search(BufferId),
    History(PathBuf),
}
fn coalesce_reads(jobs: &mut std::collections::VecDeque<IoJob>, events: &mpsc::Sender<IoEvent>) {
    let mut seen = std::collections::BTreeSet::new();
    let mut keep = vec![true; jobs.len()];
    for (i, job) in jobs.iter().enumerate().rev() {
        let key = match job {
            IoJob::Open { view, .. } => Replaceable::Open(*view),
            IoJob::Search { buffer, .. } => Replaceable::Search(*buffer),
            IoJob::History { path, .. } => Replaceable::History(path.clone()),
            // No coalescing across a durable operation or project transition.
            _ => {
                seen.clear();
                continue;
            }
        };
        if !seen.insert(key) {
            keep[i] = false;
            let _ = events.send(if matches!(job, IoJob::History { .. }) {
                IoEvent::History(Ok(()))
            } else {
                IoEvent::Superseded
            });
        }
    }
    let mut i = 0;
    jobs.retain(|_| {
        let retain = keep[i];
        i += 1;
        retain
    });
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    #[test]
    fn reads_and_history_coalesce_but_never_across_a_save() {
        let open = |serial| IoJob::Open {
            serial,
            view: 1,
            path: "file".into(),
            line: 0,
            column: 0,
            reload: None,
        };
        let history = || IoJob::History {
            history: History::default(),
            path: "history".into(),
        };
        let save = IoJob::Save {
            buffer: 1,
            path: "file".into(),
            text: Rope::from_str("saved"),
            state: 0,
            quit: false,
            protected: Vec::new(),
        };
        let mut jobs = std::collections::VecDeque::from([
            open(1),
            open(2),
            history(),
            history(),
            save,
            open(3),
            open(4),
        ]);
        let (tx, rx) = mpsc::channel();
        coalesce_reads(&mut jobs, &tx);
        assert_eq!(jobs.len(), 4);
        assert!(matches!(jobs[0], IoJob::Open { serial: 2, .. }));
        assert!(matches!(jobs[1], IoJob::History { .. }));
        assert!(matches!(jobs[2], IoJob::Save { .. }));
        assert!(matches!(jobs[3], IoJob::Open { serial: 4, .. }));
        assert_eq!(
            rx.try_iter()
                .filter(|e| matches!(e, IoEvent::History(Ok(()))))
                .count(),
            1
        );
    }
}
