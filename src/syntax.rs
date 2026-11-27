//! Parsing is isolated from input/rendering; pending edits coalesce per document.
//!
//! A worker retains parser/tree state and returns byte-based highlight snapshots
//! tagged with document, revision and generation. Callers validate that identity;
//! [`project_highlights`] maps old spans through ordered edits for a current viewport
//! while parsing catches up, rather than rewriting every retained span per keystroke.
use ropey::Rope;
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
    path::Path,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
};
#[cfg(test)]
use tree_sitter::{InputEdit, Point};
use tree_sitter::{Language, Parser, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Function,
    Constant,
    Boolean,
    Operator,
    Statement,
    Preprocessor,
    Parameter,
    Property,
}
/// A syntax token over a half-open UTF-8 byte range, not buffer scalar positions.
#[derive(Clone, Debug)]
pub struct Highlight {
    pub range: Range<usize>,
    pub token: Token,
}
pub struct SyntaxResult {
    pub generation: u64,
    pub document: u64,
    pub revision: u64,
    pub highlights: Vec<Highlight>,
}
struct Job {
    generation: u64,
    document: u64,
    revision: u64,
    text: Rope,
    language: &'static str,
    changes: Vec<crate::buffer::TextChange>,
}
struct Pending {
    jobs: VecDeque<Job>,
    desired: BTreeMap<u64, (u64, u64)>,
    serial: u64,
    closed: bool,
}
type Slot = Arc<(Mutex<Pending>, Condvar)>;
pub struct SyntaxService {
    slot: Slot,
    pub results: mpsc::Receiver<SyntaxResult>,
}

pub fn language_name(path: Option<&Path>) -> &'static str {
    match path
        .and_then(Path::extension)
        .and_then(|s| s.to_str())
        .unwrap_or("")
    {
        "rs" => "Rust",
        "py" | "pyw" => "Python",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "C++",
        "json" | "jsonc" => "JSON",
        _ => "Plain text",
    }
}
fn language(name: &str) -> Option<Language> {
    Some(match name {
        "Rust" => tree_sitter_rust::LANGUAGE.into(),
        "Python" => tree_sitter_python::LANGUAGE.into(),
        "JavaScript" => tree_sitter_javascript::LANGUAGE.into(),
        "JSON" => tree_sitter_json::LANGUAGE.into(),
        "TypeScript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "TSX" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "C" => tree_sitter_c::LANGUAGE.into(),
        "C++" => tree_sitter_cpp::LANGUAGE.into(),
        _ => return None,
    })
}
impl Default for SyntaxService {
    fn default() -> Self {
        Self::new()
    }
}
impl SyntaxService {
    pub fn new() -> Self {
        let slot: Slot = Arc::new((
            Mutex::new(Pending {
                jobs: VecDeque::new(),
                desired: BTreeMap::new(),
                serial: 0,
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker_slot = Arc::clone(&slot);
        let (tx, results) = mpsc::channel();
        thread::spawn(move || {
            let mut parser = Parser::new();
            let mut previous: BTreeMap<u64, (&str, u64, Tree)> = BTreeMap::new();
            loop {
                let job = {
                    let (lock, ready) = &*worker_slot;
                    let mut pending = lock.lock().unwrap();
                    while pending.jobs.is_empty() && !pending.closed {
                        pending = ready.wait(pending).unwrap();
                    }
                    if pending.closed {
                        break;
                    }
                    pending.jobs.pop_front().unwrap()
                };
                let mut highlights = Vec::new();
                let obsolete = || {
                    let pending = worker_slot.0.lock().unwrap();
                    pending.closed
                        || pending.desired.get(&job.document)
                            != Some(&(job.revision, job.generation))
                };
                if obsolete() {
                    continue;
                }
                if let Some(lang) = language(job.language) {
                    if parser.set_language(&lang).is_err() {
                        continue;
                    }
                    let old = previous
                        .get_mut(&job.document)
                        .and_then(|(lang, revision, tree)| {
                            if *lang != job.language {
                                return None;
                            }
                            let edits: Vec<_> = job
                                .changes
                                .iter()
                                .filter(|e| e.revision > *revision)
                                .collect();
                            if edits.first().is_none_or(|e| e.revision != *revision + 1)
                                || edits.last().is_none_or(|e| e.revision != job.revision)
                            {
                                return None;
                            }
                            for edit in edits {
                                tree.edit(&edit.syntax);
                            }
                            Some(&*tree)
                        });
                    let mut progress = |_: &tree_sitter::ParseState| {
                        if obsolete() {
                            std::ops::ControlFlow::Break(())
                        } else {
                            std::ops::ControlFlow::Continue(())
                        }
                    };
                    let tree = parser.parse_with_options(
                        &mut |byte, _| {
                            if byte >= job.text.len_bytes() {
                                return &[][..];
                            }
                            let (chunk, start, _, _) = job.text.chunk_at_byte(byte);
                            &chunk.as_bytes()[byte - start..]
                        },
                        old,
                        Some(tree_sitter::ParseOptions::new().progress_callback(&mut progress)),
                    );
                    if let Some(tree) = tree {
                        highlights = collect_cancellable(&tree, &obsolete);
                        if previous.len() >= 16 && !previous.contains_key(&job.document) {
                            previous.pop_first();
                        }
                        previous.insert(job.document, (job.language, job.revision, tree));
                    } else {
                        // A cancelled parse cannot be resumed with another document.
                        parser.reset();
                        previous.remove(&job.document);
                    }
                } else {
                    previous.remove(&job.document);
                }
                if obsolete() {
                    continue;
                }
                if tx
                    .send(SyntaxResult {
                        generation: job.generation,
                        document: job.document,
                        revision: job.revision,
                        highlights,
                    })
                    .is_err()
                {
                    break;
                }
                crate::background::wake();
            }
        });
        Self { slot, results }
    }
    pub fn request(&self, document: u64, revision: u64, text: Rope, path: Option<&Path>) {
        self.request_changes(document, revision, text, path, Vec::new());
    }
    /// Supersede queued work for this document and return its new generation.
    /// `changes` must be ordered edits leading to `text`; stale results still require
    /// caller-side document/revision/generation validation before application.
    pub fn request_changes(
        &self,
        document: u64,
        revision: u64,
        text: Rope,
        path: Option<&Path>,
        changes: Vec<crate::buffer::TextChange>,
    ) -> u64 {
        let (lock, ready) = &*self.slot;
        let mut pending = lock.lock().unwrap();
        pending.serial += 1;
        let generation = pending.serial;
        pending.desired.insert(document, (revision, generation));
        pending.jobs.retain(|job| job.document != document);
        pending.jobs.push_back(Job {
            generation,
            document,
            revision,
            text,
            language: language_name(path),
            changes,
        });
        ready.notify_one();
        generation
    }
    pub fn retain_documents(&self, documents: &std::collections::BTreeSet<u64>) {
        let mut pending = self.slot.0.lock().unwrap();
        pending.desired.retain(|id, _| documents.contains(id));
        pending.jobs.retain(|job| documents.contains(&job.document));
    }
}
impl Drop for SyntaxService {
    fn drop(&mut self) {
        self.slot.0.lock().unwrap().closed = true;
        self.slot.1.notify_one();
    }
}
#[cfg(test)]
fn point(text: &Rope, byte: usize) -> Point {
    let row = text.byte_to_line(byte);
    Point::new(row, byte - text.line_to_byte(row))
}
#[cfg(test)]
fn edit_tree(tree: &mut Tree, old: &Rope, new: &Rope) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let remaining = old.len_chars().min(new.len_chars()) - prefix;
    let mut old_chars = old.chars_at(old.len_chars());
    let mut new_chars = new.chars_at(new.len_chars());
    let suffix = (0..remaining)
        .take_while(|_| old_chars.prev() == new_chars.prev())
        .count();
    let start = old.char_to_byte(prefix);
    let old_end = old.char_to_byte(old.len_chars() - suffix);
    let new_end = new.char_to_byte(new.len_chars() - suffix);
    tree.edit(&InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point(old, start),
        old_end_position: point(old, old_end),
        new_end_position: point(new, new_end),
    });
}
#[cfg(test)]
fn collect(tree: &Tree) -> Vec<Highlight> {
    collect_cancellable(tree, &|| false)
}
fn collect_cancellable(tree: &Tree, cancelled: &impl Fn() -> bool) -> Vec<Highlight> {
    let mut spans = Vec::new();
    let mut cursor = tree.walk();
    let mut visited = 0usize;
    loop {
        visited += 1;
        if visited.is_multiple_of(256) && cancelled() {
            return Vec::new();
        }
        let node = cursor.node();
        let kind = node.kind();
        let token = if kind.contains("comment") {
            Some(Token::Comment)
        } else if kind.contains("string") || kind == "char_literal" || kind == "template_string" {
            Some(Token::String)
        } else if kind.contains("integer") || kind.contains("float") || kind == "number" {
            Some(Token::Number)
        } else if kind == "type_identifier" || kind == "primitive_type" {
            Some(Token::Type)
        } else if ["true", "false", "True", "False"].contains(&kind) {
            Some(Token::Boolean)
        } else if kind == "attribute_item" || kind.starts_with("preproc_") {
            Some(Token::Preprocessor)
        } else if [
            "if", "else", "for", "while", "match", "loop", "switch", "case", "break", "continue",
            "try", "catch", "throw",
        ]
        .contains(&kind)
        {
            Some(Token::Statement)
        } else if !node.is_named()
            && [
                "=", "+", "-", "*", "/", "%", "==", "!=", ">", "<", ">=", "<=", "=>", "->", "&&",
                "||", "!", "&", "|", "^", "~", "+=", "-=", "*=", "/=",
            ]
            .contains(&kind)
        {
            Some(Token::Operator)
        } else if ["null", "none", "None", "self", "Self"].contains(&kind) {
            Some(Token::Constant)
        } else if !node.is_named() && kind.chars().next().is_some_and(char::is_alphabetic) {
            Some(Token::Keyword)
        } else if kind == "identifier"
            && node.parent().is_some_and(|p| {
                (p.kind().contains("function") || p.kind() == "call_expression")
                    && p.child_by_field_name("name")
                        .or_else(|| p.child_by_field_name("function"))
                        .is_some_and(|n| n.id() == node.id())
            })
        {
            Some(Token::Function)
        } else if matches!(kind, "field_identifier" | "property_identifier") {
            Some(Token::Property)
        } else if kind == "identifier"
            && node
                .parent()
                .is_some_and(|p| p.kind().contains("parameter"))
        {
            Some(Token::Parameter)
        } else {
            None
        };
        if let Some(token) = token {
            spans.push(Highlight {
                range: node.byte_range(),
                token,
            });
        } else if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return spans;
            }
        }
    }
}

/// Project just the old spans that could intersect a current viewport. Reverse
/// edit mapping is conservative at replacements; adjustment uses the same ordered
/// transform as the reference whole-vector implementation.
pub fn project_highlights(
    spans: &[Highlight],
    changes: &[crate::buffer::TextChange],
    viewport: Range<usize>,
) -> Vec<Highlight> {
    if viewport.is_empty() {
        return Vec::new();
    }
    let mut regions = vec![viewport.clone()];
    for change in changes.iter().rev() {
        let e = &change.syntax;
        let mut previous = Vec::with_capacity(regions.len() + 2);
        let delta = e.old_end_byte as isize - e.new_end_byte as isize;
        for range in regions {
            if range.start < e.start_byte {
                previous.push(range.start..range.end.min(e.start_byte));
            }
            if range.end > e.new_end_byte {
                previous.push(
                    range.start.max(e.new_end_byte).saturating_add_signed(delta)
                        ..range.end.saturating_add_signed(delta),
                );
            }
            if range.start <= e.new_end_byte && range.end >= e.start_byte {
                // Replaced interiors cannot retain a standalone old token.
                // Endpoint queries retain enclosing/adjacent spans, including
                // a string/comment spanning the entire edit.
                previous.push(e.start_byte..e.start_byte);
                previous.push(e.old_end_byte..e.old_end_byte);
            }
        }
        previous.sort_unstable_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in previous {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        regions = merged;
    }
    let mut visible = Vec::new();
    let mut copied_until = 0;
    for region in regions {
        let start = spans
            .partition_point(|h| h.range.end < region.start)
            .max(copied_until);
        let end = spans.partition_point(|h| h.range.start <= region.end);
        if start < end {
            visible.extend_from_slice(&spans[start..end]);
        }
        copied_until = copied_until.max(end);
    }
    adjust_highlights(&mut visible, changes);
    visible.retain(|h| h.range.start < viewport.end && h.range.end > viewport.start);
    visible
}

/// Keep unaffected spans visible until the worker returns the new syntax tree.
pub fn adjust_highlights(spans: &mut Vec<Highlight>, changes: &[crate::buffer::TextChange]) {
    for change in changes {
        let edit = &change.syntax;
        let delta = edit.new_end_byte as isize - edit.old_end_byte as isize;
        spans.retain_mut(|h| {
            if h.range.end <= edit.start_byte {
                return true;
            }
            if h.range.start >= edit.old_end_byte {
                h.range.start = h.range.start.saturating_add_signed(delta);
                h.range.end = h.range.end.saturating_add_signed(delta);
                return true;
            }
            if h.range.start <= edit.start_byte && h.range.end >= edit.old_end_byte {
                h.range.end = h.range.end.saturating_add_signed(delta);
                return h.range.start < h.range.end;
            }
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "release reference-versus-viewport highlight benchmark"]
    fn highlight_projection_latency() {
        let source = "x ".repeat(100_000);
        let mut buffer = crate::buffer::Buffer::new(Rope::from_str(&source), None);
        let spans: Vec<_> = (0..100_000)
            .map(|i| Highlight {
                range: i * 2..i * 2 + 1,
                token: Token::Keyword,
            })
            .collect();
        for _ in 0..32 {
            buffer.replace(0..0, "é\n");
        }
        let edits = buffer.recent_changes();
        let viewport = 100_000..100_400;
        let mut full = Vec::new();
        let mut projected = Vec::new();
        for _ in 0..100 {
            let mut reference = spans.clone(); // Fixture cloning excluded.
            let at = std::time::Instant::now();
            adjust_highlights(&mut reference, &edits);
            full.push(at.elapsed().as_nanos());
            let at = std::time::Instant::now();
            let result = project_highlights(&spans, &edits, viewport.clone());
            projected.push(at.elapsed().as_nanos());
            assert_eq!(
                result.len(),
                reference
                    .iter()
                    .filter(|h| h.range.start < viewport.end && h.range.end > viewport.start)
                    .count()
            );
        }
        for (name, mut samples) in [("whole_vector", full), ("viewport_projection", projected)] {
            samples.sort_unstable();
            println!(
                "{name}: n=100 p50_us={:.2} p95_us={:.2} p99_us={:.2}",
                samples[49] as f64 / 1000.,
                samples[94] as f64 / 1000.,
                samples[98] as f64 / 1000.
            );
        }
    }
    #[test]
    fn viewport_projection_matches_ordered_reference_after_unicode_multiline_edits() {
        let source = "fn main() { let value = \"é👩‍💻\"; /* comment */ }\n".repeat(200);
        let mut buffer = crate::buffer::Buffer::new(Rope::from_str(&source), None);
        let mut parser = Parser::new();
        parser.set_language(&language("Rust").unwrap()).unwrap();
        let original = collect(&parser.parse(&source, None).unwrap());
        let mut seed = 91usize;
        for i in 0..64 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let at = seed % buffer.text.len_chars();
            let end = (at + i % 7).min(buffer.text.len_chars());
            buffer.replace(at..end, ["x", "\n日本\n", "", "e\u{301}"][i % 4]);
            let changes = buffer.recent_changes();
            let mut expected = original.clone();
            adjust_highlights(&mut expected, &changes);
            for start in [
                0,
                buffer.text.len_bytes() / 2,
                buffer.text.len_bytes().saturating_sub(200),
            ] {
                let viewport = start..(start + 200).min(buffer.text.len_bytes());
                let actual = project_highlights(&original, &changes, viewport.clone());
                let expected: Vec<_> = expected
                    .iter()
                    .filter(|h| h.range.start < viewport.end && h.range.end > viewport.start)
                    .map(|h| (&h.range, h.token))
                    .collect();
                assert_eq!(
                    actual
                        .iter()
                        .map(|h| (&h.range, h.token))
                        .collect::<Vec<_>>(),
                    expected,
                    "edit {i}, viewport {viewport:?}"
                );
            }
        }
        buffer.undo();
        assert!(buffer.changes_since(0).is_none());
        buffer.redo();
        assert!(buffer.changes_since(0).is_none());
    }
    #[test]
    fn cancellation_does_not_resume_another_documents_parse() {
        let service = SyntaxService::new();
        service.request(
            1,
            0,
            Rope::from_str(&"fn x() {}\n".repeat(100_000)),
            Some(Path::new("a.rs")),
        );
        service.request(1, 1, Rope::from_str("let x = 1;"), Some(Path::new("a.rs")));
        service.request(
            2,
            0,
            Rope::from_str("{\"a\": true}"),
            Some(Path::new("b.json")),
        );
        let mut found = std::collections::BTreeSet::new();
        while found.len() < 2 {
            let result = service
                .results
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            if result.document == 1 && result.revision == 0 {
                continue;
            }
            assert!(!result.highlights.is_empty());
            found.insert((result.document, result.revision));
        }
        assert!(found.contains(&(1, 1)) && found.contains(&(2, 0)));
    }
    #[test]
    fn recorded_edits_match_fresh_highlights() {
        let source = "fn main() { let x = \"é\"; }\n";
        let mut buffer = crate::buffer::Buffer::new(Rope::from_str(source), None);
        let service = SyntaxService::new();
        service.request_changes(
            1,
            0,
            buffer.text.clone(),
            Some(Path::new("test.rs")),
            Vec::new(),
        );
        service
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let at = source[..source.find('é').unwrap()].chars().count();
        buffer.replace(at..at + 1, "日本");
        buffer.replace(0..0, "// hello\n");
        service.request_changes(
            1,
            buffer.revision,
            buffer.text.clone(),
            Some(Path::new("test.rs")),
            buffer.recent_changes(),
        );
        let incremental = service
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let fresh = SyntaxService::new();
        fresh.request(
            1,
            buffer.revision,
            buffer.text.clone(),
            Some(Path::new("test.rs")),
        );
        let fresh = fresh
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            incremental
                .highlights
                .iter()
                .map(|h| (&h.range, h.token))
                .collect::<Vec<_>>(),
            fresh
                .highlights
                .iter()
                .map(|h| (&h.range, h.token))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn target_language_grammars_parse_unicode_and_embedded_syntax() {
        for (file, source) in [
            (
                "a.ts",
                "interface Thing { name: string }; const café: Thing = { name: '日本' };",
            ),
            (
                "a.tsx",
                "const View = () => <div title=\"日本\">{42}</div>;",
            ),
            ("a.jsx", "const View = () => <div>é</div>;"),
            ("a.c", "// 日本\nint main(void) { return 0; }"),
            (
                "a.cpp",
                "struct Thing { int field; }; int main() { Thing x; return x.field; }",
            ),
            ("a.jsonc", "// comment\n{\"é\": true}"),
        ] {
            let mut parser = Parser::new();
            let name = language_name(Some(Path::new(file)));
            parser.set_language(&language(name).unwrap()).unwrap();
            let tree = parser.parse(source, None).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{file}: {}",
                tree.root_node().to_sexp()
            );
            let highlights = collect(&tree);
            assert!(!highlights.is_empty(), "{file}");
            assert!(
                highlights
                    .iter()
                    .all(|h| source.is_char_boundary(h.range.start)
                        && source.is_char_boundary(h.range.end))
            );
        }
    }
    #[test]
    fn real_grammars_and_incremental_edits() {
        for name in ["Rust", "Python", "JavaScript", "JSON"] {
            let mut parser = Parser::new();
            parser.set_language(&language(name).unwrap()).unwrap();
            let text = if name == "JSON" {
                "{\"a\": 42}"
            } else {
                "// hello\nfn main() { let x = 42; }"
            };
            let tree = parser.parse(text, None).unwrap();
            assert!(!collect(&tree).is_empty());
        }
        let mut parser = Parser::new();
        parser.set_language(&language("Rust").unwrap()).unwrap();
        let old = Rope::from_str("fn main() { /* café */ }");
        let new = Rope::from_str("fn main() { /* 日本 */ let x = 2; }");
        let mut tree = parser.parse(old.to_string(), None).unwrap();
        edit_tree(&mut tree, &old, &new);
        let updated = parser.parse(new.to_string(), Some(&tree)).unwrap();
        let fresh = parser.parse(new.to_string(), None).unwrap();
        assert_eq!(updated.root_node().to_sexp(), fresh.root_node().to_sexp());
    }
}
