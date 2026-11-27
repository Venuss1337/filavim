//! Debounced scheduling for the existing document-search jobs. No file busy flag.
use super::*;
use std::sync::atomic::Ordering;
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Key {
    document: u64,
    revision: u64,
    query: String,
    options: crate::search::Options,
}
impl Key {
    fn current(documents: &super::super::documents::Documents) -> Self {
        let editor = &documents.editor;
        Self {
            document: documents.id(),
            revision: editor.buffer.revision,
            query: editor.last_search.clone(),
            options: editor.search_options,
        }
    }
}
impl EditorView {
    pub(super) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let key = Key::current(&self.model);
        if self.search_key.as_ref() == Some(&key) {
            return;
        }
        let editor = &mut self.model.editor;
        if editor.search_projected_revision != editor.buffer.revision {
            if let Some(changes) = editor
                .buffer
                .changes_since(editor.search_projected_revision)
            {
                crate::search::project(&mut editor.search_ranges, &changes);
            } else {
                editor.search_ranges.clear();
            }
            editor.search_projected_revision = editor.buffer.revision;
        }
        editor.search_error = false;
        if key.query.is_empty() {
            self.search_generation.fetch_add(1, Ordering::Relaxed);
            self.search_task = None;
            self.search_key = Some(key);
            editor.search_ranges.clear();
            return;
        }
        let request = Request::Search {
            text: editor.buffer.text.clone(),
            query: key.query.clone(),
            start: editor.buffer.cursor,
            revision: key.revision,
        };
        self.start_search(request, (false, None), true, cx);
    }
    pub(super) fn start_search(
        &mut self,
        request: Request,
        action: (bool, Option<(String, bool)>),
        automatic: bool,
        cx: &mut Context<Self>,
    ) {
        let key = Key::current(&self.model);
        let Some(mut job) = io::Job::from_request(request, key.document) else {
            return;
        };
        if let io::Job::Search {
            options,
            backwards,
            replacement,
            ..
        } = &mut job
        {
            *options = key.options;
            *backwards = action.0;
            *replacement = action.1;
        }
        let generation = self.search_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let token = self.search_generation.clone();
        self.search_key = Some(key.clone());
        let cursor = self.model.editor.buffer.cursor;
        let executor = cx.background_executor().clone();
        self.search_task = Some(cx.spawn(async move |view, cx| {
            if automatic {
                executor.timer(std::time::Duration::from_millis(150)).await;
            }
            let result = executor
                .spawn(async move {
                    job.run_cancellable(|| token.load(Ordering::Relaxed) != generation)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.search_generation.load(Ordering::Relaxed) != generation
                    || Key::current(&view.model) != key
                {
                    return;
                }
                match result {
                    Ok(io::Done::Search {
                        results,
                        document,
                        revision,
                    }) => {
                        view.model.editor.search_error = false;
                        if automatic
                            || view.model.editor.buffer.cursor != cursor
                                && results.replacement.is_none()
                        {
                            let editor = &mut view.model.editor;
                            editor.search_ranges = results.ranges;
                            editor.search_revision = revision;
                            editor.search_projected_revision = revision;
                        } else {
                            io::apply(
                                &mut view.model.editor,
                                io::Done::Search {
                                    results,
                                    document,
                                    revision,
                                },
                            );
                        }
                        view.cache = None;
                        view.changed(cx);
                    }
                    Err(error) => {
                        view.model.editor.search_error = true;
                        view.model.editor.search_ranges.clear();
                        view.cache = None;
                        view.model
                            .editor
                            .notify(format!("Search failed: {error:#}"), true);
                        cx.notify();
                    }
                    _ => unreachable!("Only search jobs use search scheduling"),
                }
            });
        }));
    }
}
