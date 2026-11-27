//! Snapshot-based file/search jobs evaluated by GPUI's background executor.
//!
//! [`Job::run`] performs synchronous work off the UI thread. [`Done`] carries
//! document/state identity back for validation; `EditorView::request` handles tab
//! creation/reload and routes save/search results through [`apply`].
use crate::{
    buffer::Buffer,
    editor::{Editor, Request},
    files,
};
use ropey::Rope;
use std::path::PathBuf;
pub enum Job {
    Open {
        path: PathBuf,
        document: u64,
        revision: u64,
    },
    Save {
        path: PathBuf,
        text: Rope,
        expected: files::DiskState,
        protected: Vec<PathBuf>,
        state: u64,
        document: u64,
        quit: bool,
    },
    Search {
        options: crate::search::Options,
        backwards: bool,
        replacement: Option<(String, bool)>,
        text: Rope,
        query: String,
        start: usize,
        document: u64,
        revision: u64,
    },
}
pub enum Done {
    Conflict {
        path: PathBuf,
        actual: files::DiskState,
        document: u64,
    },
    Open {
        indentation: Option<crate::editing::Indentation>,
        disk_state: files::DiskState,
        path: PathBuf,
        text: Rope,
        document: u64,
        revision: u64,
    },
    Save {
        disk_state: files::DiskState,
        path: PathBuf,
        state: u64,
        document: u64,
        quit: bool,
    },
    Search {
        results: crate::search::Results,
        document: u64,
        revision: u64,
    },
}
impl Job {
    pub fn from_request(r: Request, document: u64) -> Option<Self> {
        match r {
            Request::Open { path, revision } => Some(Self::Open {
                path,
                revision,
                document,
            }),
            Request::Save {
                expected,
                path,
                text,
                state,
                quit,
            } => Some(Self::Save {
                expected,
                protected: Vec::new(),
                path,
                text,
                state,
                quit,
                document,
            }),
            Request::Search {
                text,
                query,
                start,
                revision,
            } => Some(Self::Search {
                options: Default::default(),
                backwards: false,
                replacement: None,
                text,
                query,
                start,
                revision,
                document,
            }),
            _ => None,
        }
    }
    pub fn run(self) -> anyhow::Result<Done> {
        self.run_cancellable(|| false)
    }
    pub fn run_cancellable(self, cancelled: impl Fn() -> bool) -> anyhow::Result<Done> {
        Ok(match self {
            Self::Open {
                path,
                document,
                revision,
            } => {
                let path = files::resolve_path(&path)?;
                let (text, disk_state) = files::open_tracked(&path)?;
                Done::Open {
                    indentation: crate::editing::detect(&text),
                    disk_state,
                    path,
                    text,
                    document,
                    revision,
                }
            }
            Self::Save {
                protected,
                expected,
                path,
                text,
                state,
                document,
                quit,
            } => {
                let path = files::resolve_path(&path)?;
                for other in protected {
                    if files::resolve_path(&other).is_ok_and(|p| p == path) {
                        anyhow::bail!("Save As destination belongs to another open document");
                    }
                }
                let disk_state = match files::save_checked(&path, &text, &expected) {
                    Ok(state) => state,
                    Err(error) => {
                        if let Some(conflict) = error.downcast_ref::<files::Conflict>() {
                            return Ok(Done::Conflict {
                                path,
                                actual: conflict.actual.clone(),
                                document,
                            });
                        }
                        return Err(error);
                    }
                };
                Done::Save {
                    disk_state,
                    path,
                    state,
                    document,
                    quit,
                }
            }
            Self::Search {
                options,
                backwards,
                replacement,
                text,
                query,
                start,
                document,
                revision,
            } => Done::Search {
                results: crate::search::run_cancellable(
                    &text,
                    &query,
                    options,
                    start,
                    backwards,
                    replacement
                        .as_ref()
                        .map(|(text, all)| (text.as_str(), *all)),
                    cancelled,
                )?,
                document,
                revision,
            },
        })
    }
}
/// Apply a result only to its owning editor, checking revision for open/search.
/// Saves acknowledge the written state even if newer edits exist. Returns whether
/// a requested save-and-quit may proceed with a clean buffer; it does not quit itself.
pub fn apply(editor: &mut Editor, done: Done) -> bool {
    match done {
        Done::Open {
            indentation,
            disk_state,
            path,
            text,
            document,
            revision,
        } if editor.document_id == document && editor.buffer.revision == revision => {
            let mut buffer = Buffer::new(text, Some(path));
            buffer.disk_state = Some(disk_state);
            buffer.indentation = indentation;
            editor.set_buffer(buffer);
            editor.notify("Opened", false);
        }
        Done::Save {
            disk_state,
            path,
            state,
            document,
            quit,
        } if editor.document_id == document => {
            editor.buffer.path = Some(path);
            editor.buffer.disk_state = Some(disk_state);
            editor.buffer.mark_saved(state);
            editor.notify(
                if editor.buffer.dirty() {
                    "Snapshot saved; newer edits remain unsaved"
                } else {
                    "Saved"
                },
                false,
            );
            return quit && !editor.buffer.dirty();
        }
        Done::Search {
            results,
            document,
            revision,
        } if editor.document_id == document && editor.buffer.revision == revision => {
            if let Some(text) = results.replacement {
                editor.set_mode(crate::editor::Mode::Normal);
                editor
                    .buffer
                    .replace(0..editor.buffer.text.len_chars(), &text);
                editor.buffer.cursor = results.cursor.min(editor.buffer.text.len_chars());
                editor.buffer.snap_cursor();
                editor.notify(format!("Replaced {} matches", results.replaced), false);
            } else if let Some(current) = results.current {
                editor.buffer.cursor = results.ranges[current].start;
                editor.buffer.snap_cursor();
                editor.notify(
                    format!("{} / {} matches", current + 1, results.ranges.len()),
                    false,
                );
            } else {
                editor.notify("No matches", true);
            }
            editor.search_error = false;
            editor.search_ranges = results.ranges;
            editor.search_revision = editor.buffer.revision;
            editor.search_projected_revision = editor.buffer.revision;
        }
        _ => editor.notify("Discarded obsolete file/search result", false),
    }
    false
}
