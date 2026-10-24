//! File jobs are evaluated by GPUI's existing executor, never during rendering.
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
        state: u64,
        document: u64,
        quit: bool,
    },
    Search {
        text: Rope,
        query: String,
        start: usize,
        document: u64,
        revision: u64,
    },
}
pub enum Done {
    Open {
        path: PathBuf,
        text: Rope,
        document: u64,
        revision: u64,
    },
    Save {
        path: PathBuf,
        state: u64,
        document: u64,
        quit: bool,
    },
    Search {
        position: Option<usize>,
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
                path,
                text,
                state,
                quit,
            } => Some(Self::Save {
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
        Ok(match self {
            Self::Open {
                path,
                document,
                revision,
            } => {
                let text = files::open(&path)?;
                Done::Open {
                    path,
                    text,
                    document,
                    revision,
                }
            }
            Self::Save {
                path,
                text,
                state,
                document,
                quit,
            } => {
                files::save(&path, &text)?;
                Done::Save {
                    path,
                    state,
                    document,
                    quit,
                }
            }
            Self::Search {
                text,
                query,
                start,
                document,
                revision,
            } => Done::Search {
                position: files::find(&text, &query, start),
                document,
                revision,
            },
        })
    }
}
/// Completion identity matters even though there is only one IO job in flight.
/// Editing/new-buffer operations remain possible while that job runs.
pub fn apply(editor: &mut Editor, done: Done) -> bool {
    match done {
        Done::Open {
            path,
            text,
            document,
            revision,
        } if editor.document_id == document && editor.buffer.revision == revision => {
            editor.set_buffer(Buffer::new(text, Some(path)));
            editor.notify("Opened", false);
        }
        Done::Save {
            path,
            state,
            document,
            quit,
        } if editor.document_id == document => {
            editor.buffer.path = Some(path);
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
            position,
            document,
            revision,
        } if editor.document_id == document && editor.buffer.revision == revision => {
            if let Some(at) = position {
                editor.buffer.cursor = at;
                editor.buffer.snap_cursor();
                editor.notify("Match", false);
            } else {
                editor.notify("No match", true);
            }
        }
        _ => editor.notify("Discarded obsolete file/search result", false),
    }
    false
}
