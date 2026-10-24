//! Project indexing, fuzzy ranking, regex grep, directory rows, and bounded previews.
//! Every operation in this module is intended for a background worker.
use crate::{
    background::{Cancel, LatestWorker},
    config::NavigationConfig,
    input::Action,
};
use grep_matcher::Matcher as _;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks};
use ignore::WalkBuilder;
use nucleo::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Buffer(u64),
    Action(Action),
    Project(PathBuf),
    Command(String),
}
#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub target: Target,
    pub line: usize,
    pub column: usize,
    pub detail: String,
}
impl Item {
    pub fn file(path: PathBuf, label: String) -> Self {
        Self {
            label,
            target: Target::File(path),
            line: 0,
            column: 0,
            detail: String::new(),
        }
    }
    pub fn path(&self) -> Option<&Path> {
        match &self.target {
            Target::File(p) => Some(p),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Match {
    pub item: Item,
    pub indices: Vec<u32>,
    pub score: u32,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub directory: bool,
    pub label: String,
}
#[derive(Default)]
pub struct ProjectIndex {
    pub root: PathBuf,
    pub files: Arc<Vec<Item>>,
    pub entries: Vec<Entry>,
    pub children: BTreeMap<PathBuf, Vec<usize>>,
    pub skipped: usize,
    pub truncated: bool,
}
pub fn build_index(
    root: PathBuf,
    config: &NavigationConfig,
    cancel: impl Fn() -> bool,
) -> ProjectIndex {
    let mut index = ProjectIndex {
        root: root.clone(),
        ..ProjectIndex::default()
    };
    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(!config.show_hidden)
        .git_ignore(config.respect_ignore)
        .git_global(config.respect_ignore)
        .git_exclude(config.respect_ignore)
        .ignore(config.respect_ignore)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    for result in builder.build() {
        if cancel() {
            break;
        }
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => {
                index.skipped += 1;
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(kind) = entry.file_type() else {
            continue;
        };
        let directory = kind.is_dir();
        if !directory && !kind.is_file() && !kind.is_symlink() {
            continue;
        }
        // Do not traverse directory links, nor index them as files.
        if kind.is_symlink() && !entry.path().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let relative = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        let label = relative.to_string_lossy().replace('\\', "/");
        if !directory {
            files.push(Item::file(path.clone(), label.clone()));
        }
        index.entries.push(Entry {
            path,
            relative,
            directory,
            label,
        });
        if index.entries.len() >= config.max_files {
            index.truncated = true;
            break;
        }
    }
    files.sort_by(|a, b| a.label.cmp(&b.label));
    index.files = Arc::new(files);
    for (i, entry) in index.entries.iter().enumerate() {
        index
            .children
            .entry(
                entry
                    .relative
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf(),
            )
            .or_default()
            .push(i);
    }
    for children in index.children.values_mut() {
        children.sort_by(|a, b| {
            let (a, b) = (&index.entries[*a], &index.entries[*b]);
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
    }
    index
}
pub fn rank(items: &[Item], query: &str, limit: usize, cancel: impl Fn() -> bool) -> Vec<Match> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(nucleo::Config::DEFAULT.match_paths());
    let mut chars = Vec::new();
    let mut scores = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i % 64 == 0 && cancel() {
            return Vec::new();
        }
        if let Some(score) = pattern.score(Utf32Str::new(&item.label, &mut chars), &mut matcher) {
            let basename = match &item.target {
                Target::File(path) => path.file_stem().and_then(|s| s.to_str()),
                _ => None,
            };
            let bonus = if !query.is_empty()
                && basename.is_some_and(|name| name.eq_ignore_ascii_case(query))
            {
                64
            } else {
                0
            };
            scores.push((i, score + bonus));
        }
    }
    // Stable input order resolves ties: MRU for recents/buffers, path order for files.
    scores.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scores.truncate(limit);
    scores
        .into_iter()
        .map(|(i, score)| {
            let mut indices = Vec::new();
            pattern.indices(
                Utf32Str::new(&items[i].label, &mut chars),
                &mut matcher,
                &mut indices,
            );
            indices.sort_unstable();
            indices.dedup();
            Match {
                item: items[i].clone(),
                indices,
                score,
            }
        })
        .collect()
}
pub enum SearchJob {
    Fuzzy {
        items: Arc<Vec<Item>>,
        query: String,
        limit: usize,
    },
    Grep {
        index: Arc<ProjectIndex>,
        query: String,
        config: NavigationConfig,
    },
}
#[derive(Default)]
pub struct SearchResults {
    pub matches: Vec<Match>,
    pub message: String,
}
pub fn search(job: SearchJob, cancel: &Cancel) -> SearchResults {
    match job {
        SearchJob::Fuzzy {
            items,
            query,
            limit,
        } => SearchResults {
            matches: rank(&items, &query, limit, || cancel.cancelled()),
            message: String::new(),
        },
        SearchJob::Grep {
            index,
            query,
            config,
        } => grep(&index, &query, &config, || cancel.cancelled()),
    }
}
pub fn grep(
    index: &ProjectIndex,
    query: &str,
    config: &NavigationConfig,
    cancel: impl Fn() -> bool,
) -> SearchResults {
    if query.is_empty() {
        return SearchResults {
            message: "Type a regex to search the project".into(),
            ..SearchResults::default()
        };
    }
    let matcher = match RegexMatcherBuilder::new().case_smart(true).build(query) {
        Ok(m) => m,
        Err(e) => {
            return SearchResults {
                message: format!("Invalid regex: {e}"),
                ..SearchResults::default()
            };
        }
    };
    let mut results = SearchResults::default();
    let mut skipped = 0;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(0))
        .heap_limit(Some(2 * 1024 * 1024))
        .build();
    for item in index.files.iter() {
        if cancel() || results.matches.len() >= config.max_results {
            break;
        }
        let Some(path) = item.path() else {
            continue;
        };
        match path.metadata() {
            Ok(m) if m.is_file() && m.len() <= config.grep_max_bytes => {}
            _ => {
                skipped += 1;
                continue;
            }
        }
        let result = searcher.search_path(
            &matcher,
            path,
            sinks::UTF8(|line_number, line| {
                if cancel() || results.matches.len() >= config.max_results {
                    return Ok(false);
                }
                let line = line.trim_end_matches(['\r', '\n']);
                let mut ranges = Vec::new();
                let _ = matcher.find_iter(line.as_bytes(), |m| {
                    ranges.push(m.start()..m.end());
                    true
                });
                let first = ranges.first().map_or(0, |r| r.start);
                let column = line[..first].chars().count();
                // Keep a useful excerpt around late matches on very long lines.
                let excerpt_start = line[..first]
                    .char_indices()
                    .rev()
                    .nth(79)
                    .map_or(0, |(i, _)| i);
                let excerpt_end = line[excerpt_start..]
                    .char_indices()
                    .nth(350)
                    .map_or(line.len(), |(i, _)| excerpt_start + i);
                let excerpt = &line[excerpt_start..excerpt_end];
                let prefix = format!("{}:{}:{}  ", item.label, line_number, column + 1);
                let mut indices = Vec::new();
                let prefix_len = prefix.graphemes(true).count() as u32;
                for (i, (byte, g)) in excerpt.grapheme_indices(true).enumerate() {
                    if ranges.iter().any(|r| {
                        r.start < excerpt_start + byte + g.len() && r.end > excerpt_start + byte
                    }) {
                        indices.push(prefix_len + i as u32);
                    }
                }
                results.matches.push(Match {
                    item: Item {
                        label: format!("{prefix}{excerpt}"),
                        target: item.target.clone(),
                        line: line_number.saturating_sub(1) as usize,
                        column,
                        detail: String::new(),
                    },
                    indices,
                    score: 0,
                });
                Ok(true)
            }),
        );
        if result.is_err() {
            skipped += 1;
        }
    }
    results.message = format!(
        "{} matches{} · {skipped} skipped",
        results.matches.len(),
        if results.matches.len() >= config.max_results {
            " (limit)"
        } else {
            ""
        }
    );
    results
}
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
}
pub struct TreeJob {
    pub index: Arc<ProjectIndex>,
    pub expanded: BTreeSet<PathBuf>,
}
pub fn tree(job: TreeJob, cancel: &Cancel) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut stack = job
        .index
        .children
        .get(Path::new(""))
        .cloned()
        .unwrap_or_default();
    stack.reverse();
    while let Some(i) = stack.pop() {
        if cancel.cancelled() {
            break;
        }
        let entry = &job.index.entries[i];
        let expanded = entry.directory && job.expanded.contains(&entry.relative);
        rows.push(TreeRow {
            entry: entry.clone(),
            depth: entry.relative.components().count().saturating_sub(1),
            expanded,
        });
        if expanded && let Some(children) = job.index.children.get(&entry.relative) {
            stack.extend(children.iter().rev().copied());
        }
    }
    rows
}
pub struct PreviewJob {
    pub item: Item,
    pub query: String,
    pub max_bytes: u64,
    pub text: Option<ropey::Rope>,
}
#[derive(Default)]
pub struct Preview {
    pub title: String,
    pub lines: Vec<String>,
    pub first_line: usize,
    pub focused_line: usize,
    pub ranges: Vec<(usize, Range<usize>)>,
    pub message: String,
    pub textual: bool,
}
pub fn preview(job: PreviewJob, cancel: &Cancel) -> Preview {
    let mut result = Preview {
        title: job
            .item
            .path()
            .and_then(Path::file_name)
            .map_or_else(|| job.item.label.clone(), |p| p.to_string_lossy().into()),
        focused_line: job.item.line,
        ..Preview::default()
    };
    if let Some(text) = job.text {
        result.first_line = job
            .item
            .line
            .saturating_sub(15)
            .min(text.len_lines().saturating_sub(1));
        result.lines = text
            .lines_at(result.first_line)
            .take(60)
            .map(|l| {
                l.chars()
                    .take(500)
                    .collect::<String>()
                    .trim_end_matches(['\r', '\n'])
                    .to_owned()
            })
            .collect();
    } else if let Some(path) = job.item.path() {
        let mut bytes = Vec::new();
        let read = File::open(path).and_then(|f| f.take(job.max_bytes + 1).read_to_end(&mut bytes));
        if let Err(e) = read {
            result.message = format!("Preview unavailable: {e}");
            return result;
        }
        if cancel.cancelled() {
            return result;
        }
        if bytes.contains(&0) {
            result.message = "Binary file — preview disabled".into();
            return result;
        }
        let truncated = bytes.len() as u64 > job.max_bytes;
        bytes.truncate(job.max_bytes as usize);
        let text = String::from_utf8_lossy(&bytes);
        let start = job.item.line.saturating_sub(15);
        result.first_line = start;
        result.lines = text
            .lines()
            .skip(start)
            .take(60)
            .map(|line| line.chars().take(500).collect())
            .collect();
        if truncated {
            result.message = "Preview size limit; open the file for full contents".into();
        }
        if result.lines.is_empty() && job.item.line > 0 {
            result.message = "Match lies beyond the preview size limit".into();
        }
    } else {
        result.textual = true;
        result.lines = job.item.detail.lines().map(str::to_owned).collect();
    }
    if !job.query.is_empty()
        && let Ok(matcher) = RegexMatcherBuilder::new()
            .case_smart(true)
            .build(&job.query)
    {
        for (i, line) in result.lines.iter().enumerate() {
            let _ = matcher.find_iter(line.as_bytes(), |m| {
                result.ranges.push((i, m.start()..m.end()));
                true
            });
        }
    }
    result
}
pub struct NavigationWorkers {
    pub index: LatestWorker<(PathBuf, NavigationConfig), Arc<ProjectIndex>>,
    pub search: LatestWorker<SearchJob, SearchResults>,
    pub preview: LatestWorker<PreviewJob, Preview>,
    pub tree: LatestWorker<TreeJob, Vec<TreeRow>>,
}
impl Default for NavigationWorkers {
    fn default() -> Self {
        Self {
            index: LatestWorker::new(|(root, config), cancel| {
                Arc::new(build_index(root, &config, || cancel.cancelled()))
            }),
            search: LatestWorker::new(search),
            preview: LatestWorker::new(preview),
            tree: LatestWorker::new(tree),
        }
    }
}
