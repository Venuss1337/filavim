//! A completion list belongs to a typing context, not to one request revision.
//!
//! Sessions keep provider snapshots across compatible prefix edits; request
//! tracking/publication lives in [`super::services`]. Candidate ranges are rebased
//! through the buffer journal before use. This module neither sends LSP messages
//! nor mounts GPUI elements, so matching and lifetime rules can be tested headlessly.
use super::services::{Choice, Row};
use crate::{
    buffer::Buffer,
    lsp::{self, Encoding},
};
use ropey::Rope;
use serde_json::Value;
use std::collections::BTreeMap;
pub(super) const LOCAL_TYPES: &str = "local:typescript-types";

pub fn identifier(c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || !c.is_control()
            && !c.is_whitespace()
            && unicode_width::UnicodeWidthChar::width(c) == Some(0)
}
pub fn prefix(buffer: &Buffer) -> usize {
    let mut start = buffer.cursor;
    while start > 0 && buffer.cursor - start < 256 {
        let c = buffer.text.char(start - 1);
        if !identifier(c) {
            break;
        }
        start -= 1;
    }
    start
}
/// One provider's candidates plus the snapshot/encoding needed to interpret edits.
pub struct Source {
    pub rows: Vec<Row>,
    pub text: Rope,
    pub revision: u64,
    pub encoding: Encoding,
    pub incomplete: bool,
    pub query: String,
}
impl Source {
    pub fn reusable(&self, buffer: &Buffer, query: &str) -> bool {
        buffer.changes_since(self.revision).is_some()
            && (self.revision == buffer.revision
                || !self.rows.is_empty() && !self.incomplete && query.starts_with(&self.query))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Awaiting,
    Visible,
    Empty,
}
/// Stable typing-context identity; pending requests do not determine visibility.
/// Provider snapshots may predate `revision`; they must pass compatibility/rebasing.
pub struct Session {
    pub lifecycle: Lifecycle,
    pub id: u64,
    pub document: u64,
    path: Option<std::path::PathBuf>,
    pub mode: crate::editor::Mode,
    pub anchor: usize,
    pub revision: u64,
    pub cursor: usize,
    pub query: String,
    pub sources: BTreeMap<String, Source>,
    pub local_checked: bool,
    #[cfg(feature = "perf")]
    pub started: std::time::Instant,
    #[cfg(feature = "perf")]
    pub presented: bool,
}
impl Session {
    pub fn new(id: u64, document: u64, buffer: &Buffer, mode: crate::editor::Mode) -> Self {
        let anchor = prefix(buffer);
        Self {
            lifecycle: Lifecycle::Awaiting,
            id,
            document,
            path: buffer.path.clone(),
            mode,
            anchor,
            revision: buffer.revision,
            cursor: buffer.cursor,
            query: buffer.text.slice(anchor..buffer.cursor).to_string(),
            sources: BTreeMap::new(),
            local_checked: false,
            #[cfg(feature = "perf")]
            started: std::time::Instant::now(),
            #[cfg(feature = "perf")]
            presented: false,
        }
    }
    /// Accept only same-path/document prefix edits with complete retained history.
    /// Cursor moves, nonidentifier edits and journal loss can end reuse.
    pub fn compatible(&self, document: u64, buffer: &Buffer) -> bool {
        document == self.document
            && buffer.path == self.path
            && prefix(buffer) == self.anchor
            && buffer.cursor >= self.anchor
            && buffer.changes_since(self.revision).is_some_and(|changes| {
                let mut end = self.cursor;
                changes.iter().all(|c| {
                    let valid = c.range.start >= self.anchor
                        && c.range.end <= end
                        && c.text.chars().all(identifier);
                    end = end.saturating_add_signed(
                        c.text.chars().count() as isize - c.range.len() as isize,
                    );
                    valid
                }) && end == buffer.cursor
            })
    }
    pub fn advance(&mut self, buffer: &Buffer) {
        self.revision = buffer.revision;
        self.cursor = buffer.cursor;
        self.query = buffer.text.slice(self.anchor..self.cursor).to_string();
        // Local keywords contain no server ranges; don't retain an obsolete snapshot
        // or depend on a long edit journal to insert the current identifier prefix.
        if let Some(local) = self.sources.get_mut(LOCAL_TYPES) {
            local.revision = buffer.revision;
            local.text = buffer.text.clone();
        }
    }
    pub fn rows(&self, buffer: &Buffer, matching: &str) -> Vec<Row> {
        let timer = crate::perf::Timer::start();
        let query = self.query.to_lowercase();
        crate::perf::count(
            "completion.candidates",
            self.sources.values().map(|s| s.rows.len() as u64).sum(),
        );
        let mut matches = Vec::new();
        for (server, source) in &self.sources {
            for (index, row) in source.rows.iter().enumerate() {
                let Choice::Completion(value) = &row.choice else {
                    continue;
                };
                let label = value["filterText"]
                    .as_str()
                    .or(value["label"].as_str())
                    .unwrap_or("");
                if let Some(rank) = score(
                    value["filavimFilter"].as_str().unwrap_or(label),
                    value["filavimInitials"].as_str().unwrap_or(""),
                    &query,
                    matching,
                ) {
                    matches.push((
                        rank,
                        !value["preselect"].as_bool().unwrap_or(false),
                        value["sortText"].as_str().unwrap_or(label),
                        server,
                        index,
                    ));
                }
            }
        }
        let compare =
            |a: &(u32, bool, &str, &String, usize), b: &(u32, bool, &str, &String, usize)| a.cmp(b);
        if matches.len() > 200 {
            matches.select_nth_unstable_by(200, compare);
            matches.truncate(200);
        }
        matches.sort_unstable_by(compare);
        let journals: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|(key, source)| (key, buffer.changes_since(source.revision)))
            .collect();
        let mut rows: Vec<_> = matches
            .into_iter()
            .filter_map(|(_, _, _, server, index)| {
                let source = &self.sources[server];
                let mut row = source.rows[index].clone();
                if let Choice::Completion(value) = &mut row.choice {
                    if source.revision != buffer.revision {
                        rebase_changes(value, source, buffer, journals[server].as_deref()?).ok()?;
                    }
                    value["filavimProvider"] = Value::String(server.clone());
                    value["filavimCandidate"] = Value::String(format!(
                        "{server}:{}",
                        value["filavimKey"].as_str().unwrap_or("")
                    ));
                }
                Some(row)
            })
            .collect();
        // Deduplicate after range validation, so an unusable server item cannot
        // suppress a safe local keyword. This scans only the bounded visible list.
        let duplicates: Vec<_> = rows.iter().enumerate().filter_map(|(i, row)| {
            let Choice::Completion(value) = &row.choice else { return None };
            (value["filavimProvider"] == LOCAL_TYPES && rows.iter().any(|other| {
                matches!(&other.choice, Choice::Completion(other) if other["filavimProvider"] != LOCAL_TYPES && other["label"] == value["label"])
            })).then_some(i)
        }).collect();
        let mut index = 0;
        rows.retain(|_| {
            let keep = !duplicates.contains(&index);
            index += 1;
            keep
        });
        timer.finish("completion.filter");
        rows
    }
}
fn score(lower: &str, initials: &str, query: &str, matching: &str) -> Option<u32> {
    if lower.starts_with(query) {
        return Some(0);
    }
    if matching == "prefix" {
        return None;
    }
    if !initials.is_empty() && initials.starts_with(query) {
        return Some(1);
    }
    if lower.contains(query) {
        return Some(2);
    }
    if matching == "substring" {
        return None;
    }
    let mut chars = lower.chars();
    query
        .chars()
        .all(|q| chars.by_ref().any(|c| c == q))
        .then_some(3)
}
// Convert using the response's snapshot, then project through the edit journal.
// Edits overlapping unrelated additional edits are rejected, never guessed.
pub(super) fn rebase(value: &mut Value, source: &Source, buffer: &Buffer) -> anyhow::Result<()> {
    if source.revision == buffer.revision {
        return Ok(());
    }
    let changes = buffer
        .changes_since(source.revision)
        .ok_or_else(|| anyhow::anyhow!("Expired completion"))?;
    rebase_changes(value, source, buffer, &changes)
}
fn rebase_changes(
    value: &mut Value,
    source: &Source,
    buffer: &Buffer,
    changes: &[crate::buffer::TextChange],
) -> anyhow::Result<()> {
    let transform = |range: &mut Value, primary: bool| -> anyhow::Result<()> {
        let r: lsp_types::Range = serde_json::from_value(range.clone())?;
        let mut start = lsp::offset(&source.text, r.start, source.encoding)?;
        let mut end = lsp::offset(&source.text, r.end, source.encoding)?;
        for c in changes {
            let delta = c.text.chars().count() as isize - c.range.len() as isize;
            if primary && c.range.start >= start && c.range.end <= end {
                end = end.saturating_add_signed(delta);
            } else if c.range.end <= start {
                start = start.saturating_add_signed(delta);
                end = end.saturating_add_signed(delta);
            } else {
                anyhow::ensure!(c.range.start >= end, "Overlapping stale completion edit");
            }
        }
        *range = serde_json::to_value(lsp::selection_range(
            &buffer.text,
            start..end,
            source.encoding,
        ))?;
        Ok(())
    };
    if let Some(edit) = value.get_mut("textEdit") {
        for key in ["range", "insert", "replace"] {
            if let Some(r) = edit.get_mut(key) {
                transform(r, true)?;
            }
        }
    }
    if let Some(edits) = value["additionalTextEdits"].as_array_mut() {
        for edit in edits {
            transform(&mut edit["range"], false)?;
        }
    }
    Ok(())
}
pub fn identity(row: &Row) -> Option<&str> {
    if let Choice::Completion(v) = &row.choice {
        v["filavimCandidate"].as_str()
    } else {
        None
    }
}
/// Visible-label highlighting only; indices always refer to original UTF-8 bytes.
pub fn matching_ranges(label: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut query = query.chars().flat_map(char::to_lowercase).peekable();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (at, c) in label.char_indices() {
        let Some(&wanted) = query.peek() else {
            break;
        };
        if c.to_lowercase().any(|ch| ch == wanted) {
            query.next();
            let end = at + c.len_utf8();
            if let Some(last) = ranges.last_mut()
                && last.end == at
            {
                last.end = end;
            } else {
                ranges.push(at..end);
            }
        }
    }
    if query.peek().is_some() {
        ranges.clear();
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Mode;
    use serde_json::json;
    fn row(label: &str) -> Row {
        Row {
            label: label.into(),
            detail: String::new(),
            choice: Choice::Completion(
                json!({"label":label,"filavimKey":label,"filavimFilter":label.to_lowercase(),"filavimInitials":label.chars().filter(|c|c.is_uppercase()).collect::<String>().to_lowercase(),"textEdit":{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":label}}),
            ),
        }
    }
    fn session() -> (Buffer, Session) {
        let mut b = Buffer::new(Rope::from_str("a"), None);
        b.cursor = 1;
        let mut s = Session::new(9, 1, &b, Mode::Insert);
        s.sources.insert(
            "mock".into(),
            Source {
                text: b.text.clone(),
                revision: b.revision,
                encoding: Encoding::Utf16,
                incomplete: false,
                query: "a".into(),
                rows: vec![row("abc"), row("about")],
            },
        );
        (b, s)
    }
    #[test]
    fn empty_lists_retry_only_for_new_input_and_incomplete_lists_refresh() {
        let (mut b, mut session) = session();
        let source = session.sources.get_mut("mock").unwrap();
        source.rows.clear();
        assert!(
            source.reusable(&b, "a"),
            "unchanged input must not poll/retry"
        );
        b.insert("b");
        assert!(
            !source.reusable(&b, "ab"),
            "empty old lists cannot suppress new input"
        );
        source.rows.push(row("abc"));
        assert!(source.reusable(&b, "ab"));
        source.incomplete = true;
        assert!(!source.reusable(&b, "ab"));
    }
    #[test]
    fn session_survives_typing_backspace_and_rebases_explicit_edits() {
        let (mut b, mut s) = session();
        let id = s.id;
        for ch in ["b", "c"] {
            b.insert(ch);
            assert!(s.compatible(1, &b));
            s.advance(&b);
            let rows = s.rows(&b, "fuzzy");
            assert!(!rows.is_empty());
            assert_eq!(s.id, id);
        }
        let rows = s.rows(&b, "fuzzy");
        let Choice::Completion(v) = &rows[0].choice else {
            panic!()
        };
        assert_eq!(v["textEdit"]["range"]["end"]["character"], 3);
        b.backspace();
        assert!(s.compatible(1, &b));
        s.advance(&b);
        assert_eq!(s.rows(&b, "fuzzy").len(), 2);
        assert!(!s.compatible(2, &b));
        b.insert(".");
        assert!(!s.compatible(1, &b));
    }
    #[test]
    fn unicode_ranges_and_unrelated_additional_edits_are_safe() {
        let mut b = Buffer::new(Rope::from_str("😀a"), None);
        b.cursor = 2;
        let mut source = Source {
            rows: vec![],
            text: b.text.clone(),
            revision: b.revision,
            encoding: Encoding::Utf16,
            incomplete: false,
            query: "a".into(),
        };
        let mut v = json!({"textEdit":{"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},"newText":"abc"},"additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"// header\n"}]});
        b.insert("β");
        rebase(&mut v, &source, &b).unwrap();
        assert_eq!(v["textEdit"]["range"]["end"]["character"], 4);
        source.text = b.text.clone();
        source.revision = b.revision;
        b.replace(0..2, "");
        assert!(rebase(&mut v, &source, &b).is_err());
    }
    #[test]
    fn matching_camel_fuzzy_prefix_and_undo_invalidation() {
        assert_eq!(matching_ranges("ÉclairValue", "év"), vec![0..2, 7..8]);
        assert!(matching_ranges("test", "zzz").is_empty());
        assert_eq!(score("gethttpresponse", "ghr", "ghr", "fuzzy"), Some(1));
        assert_eq!(score("alphabet", "", "apt", "fuzzy"), Some(3));
        assert_eq!(score("alphabet", "", "apt", "prefix"), None);
        let mut combining = Buffer::new(Rope::from_str("e\u{301}"), None);
        combining.cursor = 2;
        assert_eq!(prefix(&combining), 0);
        let (mut renamed, original) = session();
        renamed.path = Some("renamed.rs".into());
        assert!(!original.compatible(1, &renamed));
        let (mut b, s) = session();
        b.insert("b");
        b.undo();
        assert!(!s.compatible(1, &b));
    }
    #[test]
    #[ignore = "short release local-filter measurement; excludes LSP, GPU and display"]
    fn completion_filter_latency() {
        for (count, edits) in [(1000, 0), (10000, 0), (1000, 64), (10000, 64)] {
            let (mut b, mut s) = session();
            for _ in 0..edits / 2 {
                b.insert("l");
                b.backspace();
            }
            s.advance(&b);
            let source = s.sources.get_mut("mock").unwrap();
            source.rows = (0..count)
                .map(|i| row(&format!("alphabet_{i:05}")))
                .collect();
            let mut samples = Vec::new();
            for _ in 0..100 {
                let at = std::time::Instant::now();
                assert_eq!(s.rows(&b, "fuzzy").len(), 200);
                samples.push(at.elapsed().as_nanos());
            }
            samples.sort();
            println!(
                "completion filter {count} candidates / {edits} journal edits (100 individual samples): p50={}us p95={}us p99={}us",
                samples[49] / 1000,
                samples[94] / 1000,
                samples[98] / 1000
            );
        }
    }
}
