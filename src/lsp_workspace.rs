//! Ordered workspace planning and recoverable resource commits. All disk work is
//! performed by the service worker; the UI only reconciles the committed prefix.
use super::{Encoding, Snapshot, ranges};
use std::sync::atomic::{AtomicBool, Ordering};
static NOT_CANCELLED: AtomicBool = AtomicBool::new(false);
use crate::files::{self, DiskState};
use anyhow::{Context, Result, bail, ensure};
use ropey::Rope;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

const MAX_ENTRIES: usize = 10000;
const MAX_TEXT: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct OpenDocument {
    pub id: u64,
    pub path: PathBuf,
    pub snapshot: Snapshot,
    pub baseline: Option<DiskState>,
    pub dirty: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Missing,
    File(DiskState),
    Directory(BTreeMap<String, State>, Option<[u64; 2]>),
    EmptyFile,
}
fn unsafe_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn directory_identity(path: &Path) -> Result<Option<[u64; 2]>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000);
    }
    files::file_identity(&options.open(path)?)
}
fn same_object(expected: &State, actual: &State) -> bool {
    match (expected, actual) {
        // Windows filename tunneling can change creation time during rename.
        // File identity, content, length and modification time must still match.
        (
            State::File(DiskState::Present {
                identity: a,
                bytes: ab,
                digest: ad,
                modified: am,
                ..
            }),
            State::File(DiskState::Present {
                identity: b,
                bytes: bb,
                digest: bd,
                modified: bm,
                ..
            }),
        ) => a == b && ab == bb && ad == bd && am == bm,
        (State::Directory(a, ai), State::Directory(b, bi)) => {
            ai == bi
                && a.len() == b.len()
                && a.iter()
                    .all(|(name, state)| b.get(name).is_some_and(|other| same_object(state, other)))
        }
        _ => matches_state(expected, actual),
    }
}
fn check_components(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "Workspace paths must be absolute");
    ensure!(
        !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir)),
        "Traversal components are not authorized"
    );
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                !unsafe_link(&metadata),
                "Symlink/junction/reparse point rejected: {}",
                ancestor.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn target(uri: &Value) -> Result<PathBuf> {
    let address = url::Url::parse(uri.as_str().context("Missing resource URI")?)?;
    ensure!(
        address.scheme() == "file"
            && address.query().is_none()
            && address.fragment().is_none()
            && address.username().is_empty()
            && address.password().is_none(),
        "Only unambiguous local file URIs are supported"
    );
    ensure!(
        address
            .host_str()
            .is_none_or(|h| h.is_empty() || h == "localhost"),
        "Remote file URI rejected"
    );
    // URL parsing normalizes dot components. Reject them in the supplied URI too.
    let decoded = address
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("Invalid file URI"))?;
    let raw = uri
        .as_str()
        .unwrap()
        .to_ascii_lowercase()
        .replace("%2e", ".")
        .replace('\\', "/");
    ensure!(
        !raw.contains("/../")
            && !raw.contains("/./")
            && !raw.ends_with("/..")
            && !raw.ends_with("/.")
            && !raw.contains("%5c"),
        "Encoded/traversal path rejected"
    );
    #[cfg(windows)]
    {
        let display = decoded.to_string_lossy();
        ensure!(
            !display.starts_with("\\\\") && !display[2.min(display.len())..].contains(':'),
            "UNC/device/alternate stream paths rejected"
        );
        for component in decoded.components() {
            if let Component::Normal(name) = component {
                let name = name.to_string_lossy();
                let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                ensure!(
                    !name.ends_with(['.', ' '])
                        && !matches!(
                            stem.as_str(),
                            "CON"
                                | "PRN"
                                | "AUX"
                                | "NUL"
                                | "COM1"
                                | "COM2"
                                | "COM3"
                                | "COM4"
                                | "COM5"
                                | "COM6"
                                | "COM7"
                                | "COM8"
                                | "COM9"
                                | "LPT1"
                                | "LPT2"
                                | "LPT3"
                                | "LPT4"
                                | "LPT5"
                                | "LPT6"
                                | "LPT7"
                                | "LPT8"
                                | "LPT9"
                        ),
                    "Ambiguous Windows filename rejected"
                );
            }
        }
    }
    check_components(&decoded)?;
    let mut ancestor = decoded.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .context("Invalid resource path")?
                .to_os_string(),
        );
        ancestor = ancestor.parent().context("No existing ancestor")?;
    }
    let mut resolved = PathBuf::from(crate::paths::display_path(&std::fs::canonicalize(
        ancestor,
    )?));
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}
fn scan_cancellable(path: &Path, count: &mut usize, cancel: &AtomicBool) -> Result<State> {
    ensure!(!cancel.load(Ordering::Relaxed), "Workspace edit cancelled");
    *count += 1;
    ensure!(
        *count <= MAX_ENTRIES,
        "Workspace edit exceeds {MAX_ENTRIES} filesystem entries"
    );
    check_components(path)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::Missing),
        Err(e) => return Err(e.into()),
    };
    ensure!(!unsafe_link(&metadata), "Resource link rejected");
    if metadata.is_file() {
        return Ok(State::File(files::disk_state_cancellable(path, || {
            cancel.load(Ordering::Relaxed)
        })?));
    }
    ensure!(metadata.is_dir(), "Unsupported filesystem object");
    let mut children = BTreeMap::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Non-Unicode resource name"))?;
        children.insert(name, scan_cancellable(&entry.path(), count, cancel)?);
    }
    Ok(State::Directory(children, directory_identity(path)?))
}
fn matches_state(expected: &State, actual: &State) -> bool {
    match (expected, actual) {
        (State::EmptyFile, State::File(DiskState::Present { bytes: 0, .. })) => true,
        (State::File(DiskState::Missing), State::Missing) => true,
        (State::Directory(a, ai), State::Directory(b, bi)) => {
            ai == bi
                && a.len() == b.len()
                && a.iter().all(|(name, state)| {
                    b.get(name).is_some_and(|other| matches_state(state, other))
                })
        }
        _ => expected == actual,
    }
}
fn flatten(path: &Path, state: &State, nodes: &mut BTreeMap<PathBuf, State>) {
    if *state != State::Missing {
        nodes.insert(path.into(), state.clone());
    }
    if let State::Directory(children, _) = state {
        for (name, child) in children {
            flatten(&path.join(name), child, nodes);
        }
    }
}
fn tree(path: &Path, nodes: &BTreeMap<PathBuf, State>) -> State {
    match nodes.get(path) {
        Some(State::Directory(_, id)) => State::Directory(
            nodes
                .keys()
                .filter(|p| p.parent() == Some(path))
                .map(|p| {
                    (
                        p.file_name().unwrap().to_string_lossy().into_owned(),
                        tree(p, nodes),
                    )
                })
                .collect(),
            *id,
        ),
        Some(state) => state.clone(),
        None => State::Missing,
    }
}
pub fn relative_path(path: &Path, parent: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(crate::paths::display_path(path));
    let parent = PathBuf::from(crate::paths::display_path(parent));
    let mut parts = path.components();
    for expected in parent.components() {
        let actual = parts.next()?;
        if cfg!(windows) {
            if actual.as_os_str().to_string_lossy().to_lowercase()
                != expected.as_os_str().to_string_lossy().to_lowercase()
            {
                return None;
            }
        } else if actual != expected {
            return None;
        }
    }
    Some(parts.collect())
}
fn logical_path(path: PathBuf, nodes: &BTreeMap<PathBuf, State>) -> PathBuf {
    if cfg!(windows) && !nodes.contains_key(&path) {
        let key = super::path_key(&path);
        if let Some(existing) = nodes.keys().find(|p| super::path_key(p) == key) {
            return existing.clone();
        }
    }
    path
}
#[derive(Clone)]
pub enum Operation {
    Text {
        path: PathBuf,
        before: Rope,
        edits: Vec<(std::ops::Range<usize>, String)>,
    },
    Create {
        path: PathBuf,
        overwrite: bool,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        overwrite: bool,
        case_only: bool,
    },
    Delete {
        path: PathBuf,
    },
    Ignore,
}
#[derive(Clone)]
pub struct Step {
    pub index: usize,
    pub summary: String,
    pub operation: Operation,
    expected: Vec<(PathBuf, State)>,
    parents: Vec<(PathBuf, Option<[u64; 2]>)>,
}
impl Step {
    pub fn touches(&self, path: &Path) -> bool {
        match &self.operation {
            Operation::Text { path: target, .. }
            | Operation::Create { path: target, .. }
            | Operation::Delete { path: target } => relative_path(path, target).is_some(),
            Operation::Rename { from, to, .. } => {
                relative_path(path, from).is_some() || relative_path(path, to).is_some()
            }
            Operation::Ignore => false,
        }
    }
}
pub struct Plan {
    pub steps: Vec<Step>,
    pub confirmations: Vec<String>,
    initial: BTreeMap<PathBuf, State>,
    parents: BTreeMap<PathBuf, Option<[u64; 2]>>,
}
#[derive(Debug)]
pub struct Failure {
    pub index: usize,
    pub reason: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Change {}: {}", self.index + 1, self.reason)
    }
}
impl std::error::Error for Failure {}
fn flag(options: &Value, key: &str) -> Result<bool> {
    match options.get(key) {
        None => Ok(false),
        Some(v) => v.as_bool().context("Resource option must be boolean"),
    }
}
fn annotation(value: &Value, annotations: &Value, confirmations: &mut Vec<String>) -> Result<()> {
    if let Some(id) = value.get("annotationId") {
        let id = id.as_str().context("annotationId must be a string")?;
        let annotation = annotations.get(id).context("Unknown change annotation")?;
        let label = annotation["label"]
            .as_str()
            .context("Annotation label required")?;
        ensure!(
            annotation.get("description").is_none_or(Value::is_string),
            "Invalid annotation description"
        );
        let required = flag(annotation, "needsConfirmation")?;
        if required {
            confirmations.push(format!(
                "{}{}",
                label,
                annotation["description"]
                    .as_str()
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ));
        }
    }
    Ok(())
}
pub fn needs_ordered_plan(value: &Value) -> bool {
    value["documentChanges"]
        .as_array()
        .is_some_and(|changes| changes.iter().any(|c| c.get("kind").is_some()))
        || value.get("changeAnnotations").is_some()
}
/// Resolve and validate the entire sequence without changing disk or documents.
pub fn plan(
    value: &Value,
    root: &Path,
    documents: Vec<OpenDocument>,
    encoding: Encoding,
) -> Result<Plan, Failure> {
    plan_cancellable(value, root, documents, encoding, &NOT_CANCELLED)
}
pub(crate) fn plan_cancellable(
    value: &Value,
    root: &Path,
    documents: Vec<OpenDocument>,
    encoding: Encoding,
    cancel: &AtomicBool,
) -> Result<Plan, Failure> {
    let result = (|| -> Result<Plan> {
        ensure!(!cancel.load(Ordering::Relaxed), "Workspace edit cancelled");
        ensure!(value.is_object(), "WorkspaceEdit must be an object");
        ensure!(
            value.get("changeAnnotations").is_none_or(Value::is_object),
            "changeAnnotations must be an object"
        );
        let root = PathBuf::from(crate::paths::display_path(
            &std::fs::canonicalize(root).context("Cannot resolve workspace root")?,
        ));
        let documents: Vec<_> = documents
            .into_iter()
            .map(|mut d| {
                d.path = PathBuf::from(crate::paths::display_path(&d.path));
                d
            })
            .collect();
        for (id, definition) in value["changeAnnotations"].as_object().into_iter().flatten() {
            annotation(
                &json!({"annotationId":id}),
                &json!({id:definition}),
                &mut Vec::new(),
            )?;
        }
        let changes: Vec<Value> = if let Some(changes) = value.get("documentChanges") {
            changes
                .as_array()
                .context("documentChanges must be an array")?
                .clone()
        } else if value.get("changes").is_none() {
            Vec::new()
        } else {
            value["changes"]
                .as_object()
                .context("Missing workspace changes")?
                .iter()
                .map(
                    |(uri, edits)| json!({"textDocument":{"uri":uri,"version":null},"edits":edits}),
                )
                .collect()
        };
        ensure!(changes.len() <= MAX_ENTRIES, "Too many workspace changes");
        let mut initial = BTreeMap::new();
        let mut parents = BTreeMap::new();
        let mut nodes = BTreeMap::new();
        let mut count = 0;
        for (index, change) in changes.iter().enumerate() {
            ensure!(!cancel.load(Ordering::Relaxed), "Workspace edit cancelled");
            let uris = if change.get("kind").is_some() {
                if change["kind"] == "rename" {
                    vec![&change["oldUri"], &change["newUri"]]
                } else {
                    vec![&change["uri"]]
                }
            } else {
                vec![&change["textDocument"]["uri"]]
            };
            for uri in uris {
                let path = target(uri).map_err(|e| {
                    anyhow::anyhow!(Failure {
                        index,
                        reason: format!("{e:#}")
                    })
                })?;
                if !initial.contains_key(&path) {
                    let state = scan_cancellable(&path, &mut count, cancel).map_err(|e| {
                        anyhow::anyhow!(Failure {
                            index,
                            reason: format!("{e:#}")
                        })
                    })?;
                    flatten(&path, &state, &mut nodes);
                    initial.insert(path.clone(), state);
                    for parent in path.ancestors().skip(1) {
                        if parent.is_dir() {
                            parents
                                .entry(parent.to_path_buf())
                                .or_insert(directory_identity(parent)?);
                            nodes.entry(parent.into()).or_insert_with(|| {
                                State::Directory(
                                    BTreeMap::new(),
                                    directory_identity(parent).ok().flatten(),
                                )
                            });
                        }
                    }
                }
            }
        }
        for document in &documents {
            if !nodes.contains_key(&document.path) && initial.contains_key(&document.path) {
                nodes.insert(document.path.clone(), State::File(DiskState::Missing));
            }
        }
        let mut virtual_docs: BTreeMap<PathBuf, (Rope, Option<u64>, bool)> = documents
            .iter()
            .map(|d| {
                (
                    d.path.clone(),
                    (d.snapshot.text.clone(), Some(d.snapshot.revision), d.dirty),
                )
            })
            .collect();
        let mut sources: BTreeMap<PathBuf, Option<PathBuf>> = nodes
            .iter()
            .filter(|(_, state)| matches!(state, State::File(_)))
            .map(|(p, _)| (p.clone(), Some(p.clone())))
            .collect();
        let mut steps = Vec::new();
        let mut confirmations = Vec::new();
        for (index, change) in changes.iter().enumerate() {
            ensure!(!cancel.load(Ordering::Relaxed), "Workspace edit cancelled");
            let step = (|| -> Result<Step> {
                annotation(change, &value["changeAnnotations"], &mut confirmations)?;
                let reason_start=confirmations.len();
                let mut expected = Vec::new();
                let operation;
                let summary;
                if let Some(kind) = change.get("kind") {
                    let options = change.get("options").unwrap_or(&Value::Null);
                    ensure!(options.is_null() || options.is_object(), "Invalid resource options");
                    let path = logical_path(target(if kind == "rename" { &change["oldUri"] } else { &change["uri"] })?,&nodes);
                    ensure!(path != root && path.parent().is_some(), "Cannot replace a workspace/filesystem root");
                    let before = match tree(&path,&nodes){State::File(DiskState::Missing)=>State::Missing,state=>state};
                    expected.push((path.clone(), before.clone()));
                    match kind.as_str().context("Resource kind must be a string")? {
                        "create" => {
                            let overwrite = flag(options, "overwrite")?;
                            let ignore = flag(options, "ignoreIfExists")?;
                            ensure!(!matches!(before, State::Directory(..)), "CreateFile cannot replace a directory");
                            ensure!(matches!(nodes.get(path.parent().unwrap()), Some(State::Directory(..))), "Create parent does not exist");
                            if before != State::Missing && !overwrite {
                                ensure!(ignore, "Create destination already exists");
                                operation = Operation::Ignore;
                            } else {
                                if before != State::Missing {
                                    ensure!(!virtual_docs.get(&path).is_some_and(|d| d.2), "Cannot overwrite a dirty document");
                                    confirmations.push(format!("Overwrite {} (original retained in backup)", path.display()));
                                    virtual_docs.remove(&path);
                                }
                                nodes.insert(path.clone(), State::EmptyFile);
                                sources.insert(path.clone(), None);
                                operation = Operation::Create { path: path.clone(), overwrite };
                            }
                            summary = format!("Create {}", path.display());
                        }
                        "rename" => {
                            let canonical_to = target(&change["newUri"])?;
                            let mut to=canonical_to.clone();
                            if cfg!(windows){let requested=super::path(change["newUri"].as_str().context("Missing rename URI")?)?;to.set_file_name(requested.file_name().context("Missing destination name")?);}
                            let case_only=cfg!(windows) && super::path_key(&path)==super::path_key(&to) && path!=to;
                            ensure!(case_only || (path != to && !to.starts_with(&path) && !path.starts_with(&to)), "Overlapping/self rename rejected");
                            ensure!(to != root, "Cannot replace workspace root");
                            let destination = tree(&logical_path(canonical_to,&nodes), &nodes);
                            expected.push((to.clone(), destination.clone()));
                            let overwrite = flag(options, "overwrite")?;
                            let ignore = flag(options, "ignoreIfExists")?;
                            ensure!(before != State::Missing, "Rename source missing");
                            ensure!(matches!(nodes.get(to.parent().context("Missing rename parent")?), Some(State::Directory(..))), "Rename parent missing");
                            if destination != State::Missing && !overwrite && !case_only {
                                ensure!(ignore, "Rename destination already exists");
                                operation = Operation::Ignore;
                            } else {
                                ensure!(case_only || !virtual_docs.iter().any(|(p, d)| relative_path(p,&to).is_some() && d.2), "Cannot overwrite a dirty destination document");
                                confirmations.push(format!("Rename {} → {}{}", path.display(), to.display(), if case_only {" (case change)"} else if destination != State::Missing { " (overwrite; backup retained)" } else { "" }));
                                remap(&mut nodes, &path, &to);
                                remap(&mut virtual_docs, &path, &to);
                                remap(&mut sources, &path, &to);
                                operation = Operation::Rename { from: path.clone(), to: to.clone(), overwrite, case_only };
                            }
                            if !to.starts_with(&root) { confirmations.push(format!("Outside workspace: {}", to.display())); }
                            summary = format!("Rename {} → {}", path.display(), to.display());
                        }
                        "delete" => {
                            let recursive = flag(options, "recursive")?;
                            let ignore = flag(options, "ignoreIfNotExists")?;
                            if before == State::Missing {
                                ensure!(ignore, "Delete source missing"); operation = Operation::Ignore;
                            } else {
                                ensure!(!matches!(&before, State::Directory(children,_) if !children.is_empty()) || recursive, "Directory deletion requires recursive=true");
                                confirmations.push(format!("Delete {} (backup retained; open buffers kept untitled)", path.display()));
                                nodes.retain(|p, _| !p.starts_with(&path));
                                sources.retain(|p, _| !p.starts_with(&path));
                                virtual_docs.retain(|p, _| !p.starts_with(&path));
                                operation = Operation::Delete { path: path.clone() };
                            }
                            summary = format!("Delete {}", path.display());
                        }
                        _ => bail!("Unsupported resource operation"),
                    }
                    if !path.starts_with(&root) { confirmations.push(format!("Outside workspace: {}", path.display())); }
                } else {
                    let path = logical_path(target(&change["textDocument"]["uri"])?,&nodes);
                    ensure!(matches!(nodes.get(&path), Some(State::File(_) | State::EmptyFile)), "Text target does not exist or is not a file");
                    if !virtual_docs.contains_key(&path) {
                        let text = if let Some(Some(source)) = sources.get(&path) {
                            ensure!(std::fs::metadata(source)?.len() <= MAX_TEXT as u64, "Workspace text exceeds limit");
                            files::open_tracked(source)?.0
                        } else { Rope::new() };
                        virtual_docs.insert(path.clone(), (text, None, false));
                    }
                    let (text, revision, dirty) = virtual_docs.get_mut(&path).unwrap();
                    if !change["textDocument"]["version"].is_null() {
                        let version = change["textDocument"]["version"].as_u64().context("Invalid document version")?;
                        ensure!(*revision == Some(version), "Stale/versioned unopened document edit");
                    }
                    for edit in change["edits"].as_array().context("Missing text edits")? { annotation(edit, &value["changeAnnotations"], &mut confirmations)?; }
                    let edits = ranges(text, &serde_json::from_value::<Vec<lsp_types::TextEdit>>(change["edits"].clone())?, encoding)?;
                    let before = text.clone();
                    *dirty=true;
                    for (range, replacement) in &edits { text.remove(range.clone()); text.insert(range.start, replacement); }
                    ensure!(text.len_bytes() <= MAX_TEXT, "Edited text exceeds limit");
                    expected.push((path.clone(), tree(&path, &nodes)));
                    if !path.starts_with(&root) { confirmations.push(format!("Outside workspace: {}", path.display())); }
                    summary = format!("Edit {}", path.display());
                    operation = Operation::Text { path, before, edits };
                }
                let step_parents=expected.iter().filter_map(|(p,_)|p.parent()).filter_map(|p|match nodes.get(p){Some(State::Directory(_,id))=>Some((p.to_path_buf(),*id)),_=>None}).collect();
                for reason in &mut confirmations[reason_start..]{*reason=format!("Change {}: {reason}",index+1);}
                let mut labels=Vec::new();
                for item in std::iter::once(change).chain(change["edits"].as_array().into_iter().flatten()) {
                    if let Some(id)=item["annotationId"].as_str(){let annotation=&value["changeAnnotations"][id];labels.push(format!("{}{}",annotation["label"].as_str().unwrap_or(""),annotation["description"].as_str().map(|d|format!(": {d}")).unwrap_or_default()));}
                }
                let summary=format!("{}. {summary}{}",index+1,if labels.is_empty(){String::new()}else{format!(" · {}",labels.join("; "))});
                Ok(Step { index, summary, operation, expected, parents:step_parents })
            })().map_err(|e| anyhow::anyhow!(Failure { index, reason: format!("{e:#}") }))?;
            steps.push(step);
        }
        for step in &steps {
            if !matches!(step.operation, Operation::Text { .. } | Operation::Ignore) {
                for (path, _) in &step.expected {
                    let permission_check = (|| -> Result<()> {
                        if let Ok(metadata) = std::fs::metadata(path) {
                            ensure!(
                                !metadata.permissions().readonly(),
                                "Read-only resource: {}",
                                path.display()
                            );
                        }
                        if let Some(parent) = path.ancestors().skip(1).find(|p| p.is_dir()) {
                            ensure!(
                                !std::fs::metadata(parent)?.permissions().readonly(),
                                "Read-only resource parent: {}",
                                parent.display()
                            );
                        }
                        Ok(())
                    })();
                    permission_check.map_err(|e| {
                        anyhow::anyhow!(Failure {
                            index: step.index,
                            reason: format!("{e:#}")
                        })
                    })?;
                }
            }
        }
        for document in &documents {
            if steps.iter().any(|s| match &s.operation {
                Operation::Create { path, .. } | Operation::Delete { path } => {
                    document.path.starts_with(path)
                }
                Operation::Rename { from, to, .. } => {
                    document.path.starts_with(from) || document.path.starts_with(to)
                }
                _ => false,
            }) {
                if let Some(baseline) = &document.baseline {
                    files::verify_state_cancellable(&document.path, baseline, || {
                        cancel.load(Ordering::Relaxed)
                    })?;
                } else {
                    ensure!(
                        !document.path.exists(),
                        "Open file has no verified disk baseline; reopen it before resource changes"
                    );
                }
            }
        }
        confirmations.sort();
        confirmations.dedup();
        Ok(Plan {
            steps,
            confirmations,
            initial,
            parents,
        })
    })();
    result.map_err(|e| {
        e.downcast_ref::<Failure>()
            .map(|f| Failure {
                index: f.index,
                reason: f.reason.clone(),
            })
            .unwrap_or(Failure {
                index: 0,
                reason: format!("{e:#}"),
            })
    })
}
fn remap<T>(map: &mut BTreeMap<PathBuf, T>, from: &Path, to: &Path) {
    let moving: Vec<_> = map
        .keys()
        .filter(|p| relative_path(p, from).is_some())
        .cloned()
        .collect();
    let values: Vec<_> = moving
        .into_iter()
        .map(|p| {
            let value = map.remove(&p).unwrap();
            (
                if relative_path(&p, from).is_some_and(|s| s.as_os_str().is_empty()) {
                    to.to_path_buf()
                } else {
                    to.join(relative_path(&p, from).unwrap())
                },
                value,
            )
        })
        .collect();
    map.retain(|p, _| relative_path(p, to).is_none());
    map.extend(values);
}
pub struct Applied {
    pub step: Step,
    pub baselines: BTreeMap<PathBuf, DiskState>,
    pub displaced: Vec<PathBuf>,
}
impl Applied {
    pub fn touches(&self, path: &Path) -> bool {
        self.step.touches(path)
            || self
                .displaced
                .iter()
                .any(|p| relative_path(path, p).is_some())
    }
}
pub struct Outcome {
    pub applied: Vec<Applied>,
    pub failure: Option<Failure>,
    pub backups: Vec<PathBuf>,
    pub displaced: Vec<PathBuf>,
}
impl Outcome {
    pub fn response(&self) -> Value {
        match &self.failure {
            None => json!({"applied":true}),
            Some(f) => {
                json!({"applied":false,"failureReason":format!("{f}; {} preceding changes completed; failed change may have retained filesystem effects; recovery directories: {}",self.applied.iter().filter(|a|a.step.index<f.index).count(),self.backups.iter().map(|p|p.display().to_string()).collect::<Vec<_>>().join(", ")),"failedChange":f.index})
            }
        }
    }
}
/// Prepare subscribed file-operation notifications on the commit worker. Preserve
/// operation order instead of grouping creates, renames and deletes by kind.
pub fn notifications(outcome: &Outcome, options: &Value) -> Vec<(String, Value)> {
    let mut filters: BTreeMap<&str, Vec<(globset::GlobMatcher, Option<String>)>> = BTreeMap::new();
    for kind in ["didCreate", "didRename", "didDelete"] {
        let compiled = options[kind]["filters"]
            .as_array()
            .into_iter()
            .flatten()
            .take(128)
            .filter_map(|filter| {
                if filter["scheme"].as_str().is_some_and(|s| s != "file") {
                    return None;
                }
                let pattern = &filter["pattern"];
                let glob = pattern["glob"].as_str()?;
                if glob.len() > 4096 {
                    return None;
                }
                let matcher = globset::GlobBuilder::new(glob)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .case_insensitive(pattern["options"]["ignoreCase"] == true)
                    .build()
                    .ok()?
                    .compile_matcher();
                Some((matcher, pattern["matches"].as_str().map(str::to_owned)))
            })
            .collect();
        filters.insert(kind, compiled);
    }
    let mut notifications = Vec::new();
    let mut watched = Vec::new();
    for applied in &outcome.applied {
        let (name, paths, payload) = match &applied.step.operation {
            Operation::Create { path, .. } => (
                "didCreate",
                vec![path],
                json!({"uri":super::uri(path).unwrap_or_default()}),
            ),
            Operation::Rename { from, to, .. } => (
                "didRename",
                vec![from, to],
                json!({"oldUri":super::uri(from).unwrap_or_default(),"newUri":super::uri(to).unwrap_or_default()}),
            ),
            Operation::Delete { path } => (
                "didDelete",
                vec![path],
                json!({"uri":super::uri(path).unwrap_or_default()}),
            ),
            _ => continue,
        };
        let directory = applied
            .step
            .expected
            .first()
            .is_some_and(|(_, s)| matches!(s, State::Directory(..)));
        match &applied.step.operation {
            Operation::Create { path, .. } => {
                watched.push(json!({"uri":super::uri(path).unwrap_or_default(),"type":1}))
            }
            Operation::Rename { from, to, .. } => {
                let files: Vec<_> = if directory && !applied.baselines.is_empty() {
                    applied.baselines.keys().cloned().collect()
                } else {
                    vec![to.clone()]
                };
                for path in files {
                    let old = relative_path(&path, to)
                        .filter(|p| !p.as_os_str().is_empty())
                        .map_or_else(|| from.clone(), |suffix| from.join(suffix));
                    watched.push(json!({"uri":super::uri(&old).unwrap_or_default(),"type":3}));
                    watched.push(json!({"uri":super::uri(&path).unwrap_or_default(),"type":1}));
                }
            }
            Operation::Delete { path } => {
                let mut files = BTreeMap::new();
                if directory {
                    collect_baselines(path, &applied.step.expected[0].1, &mut files);
                }
                if files.is_empty() {
                    watched.push(json!({"uri":super::uri(path).unwrap_or_default(),"type":3}));
                } else {
                    for file in files.keys() {
                        watched.push(json!({"uri":super::uri(file).unwrap_or_default(),"type":3}));
                    }
                }
            }
            _ => (),
        }
        if filters[name].iter().any(|(glob, kind)| {
            if kind
                .as_deref()
                .is_some_and(|k| k != if directory { "folder" } else { "file" })
            {
                return false;
            }
            paths
                .iter()
                .any(|path| glob.is_match(crate::paths::display_path(path).replace('\\', "/")))
        }) {
            notifications.push((format!("workspace/{name}Files"), json!({"files":[payload]})));
        }
    }
    if !watched.is_empty() {
        notifications.push((
            "workspace/didChangeWatchedFiles".into(),
            json!({"changes":watched}),
        ));
    }
    notifications
}
fn writable(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::metadata(path) {
        ensure!(
            !metadata.permissions().readonly(),
            "Read-only resource: {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        ensure!(
            !std::fs::metadata(parent)?.permissions().readonly(),
            "Read-only parent directory"
        );
    }
    Ok(())
}
#[cfg(windows)]
fn commit_guards(paths: impl Iterator<Item = PathBuf>) -> Result<Vec<std::fs::File>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    let mut directories = std::collections::BTreeSet::new();
    let mut handles = Vec::new();
    for path in paths {
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                directories.insert(parent.to_path_buf());
            }
        }
        // Hold the verified file against writers while permitting our own move.
        if path.is_file() {
            handles.push(files::save_guard(&path)?.context("Resource disappeared before commit")?);
        }
    }
    for directory in directories {
        let handle = std::fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&directory)?;
        ensure!(
            !unsafe_link(&handle.metadata()?),
            "Reparse point at resource commit"
        );
        handles.push(handle);
    }
    Ok(handles)
}
#[cfg(not(windows))]
fn commit_guards(_paths: impl Iterator<Item = PathBuf>) -> Result<Vec<std::fs::File>> {
    Ok(Vec::new())
}
fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // No REPLACE_EXISTING: an external destination inserted at the last
        // boundary must cause failure, not replace an unapproved object.
        ensure!(
            unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
            } != 0,
            "Rename failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let result = {
            unsafe extern "C" {
                fn renameat2(
                    from_fd: i32,
                    from: *const std::ffi::c_char,
                    to_fd: i32,
                    to: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) }
        };
        #[cfg(target_os = "macos")]
        let result = {
            unsafe extern "C" {
                fn renamex_np(
                    from: *const std::ffi::c_char,
                    to: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) }
        };
        ensure!(
            result == 0,
            "Rename failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, to);
        bail!("Atomic no-replace rename unavailable on this platform")
    }
}
fn backup(
    path: &Path,
    expected: &State,
    backups: &mut Vec<PathBuf>,
    displaced: &mut Vec<PathBuf>,
    cancel: &AtomicBool,
) -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix(".filavim-lsp-backup-")
        .tempdir_in(path.parent().context("No backup parent")?)?
        .keep();
    // Retain the directory even on failure: an external writer may have changed
    // the object between final verification and rename. Never destroy that object.
    backups.push(directory.clone());
    std::fs::write(
        directory.join("SOURCE.txt"),
        path.to_string_lossy().as_bytes(),
    )?;
    rename_no_replace(path, &directory.join("original"))?;
    displaced.push(path.to_path_buf());
    ensure!(
        same_object(
            expected,
            &scan_cancellable(&directory.join("original"), &mut 0, cancel)?
        ),
        "Resource changed at commit boundary; moved version preserved in backup"
    );
    Ok(())
}
fn collect_baselines(path: &Path, state: &State, output: &mut BTreeMap<PathBuf, DiskState>) {
    match state {
        State::File(state) => {
            output.insert(path.into(), state.clone());
        }
        State::Directory(children, _) => {
            for (name, state) in children {
                collect_baselines(&path.join(name), state, output);
            }
        }
        _ => (),
    }
}
/// Commit only resource changes. Text remains in undoable, unsaved editor buffers.
/// Cross-resource atomicity is not promised. Failures retain the successful prefix
/// and any moved originals; no rollback overwrites potentially newer external data.
pub fn execute(plan: Plan) -> Outcome {
    execute_with_progress(plan, &NOT_CANCELLED, |_, _| {})
}
pub fn execute_with_progress(
    plan: Plan,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Outcome {
    let total = plan.steps.len();
    let mut outcome = Outcome {
        applied: Vec::new(),
        failure: None,
        backups: Vec::new(),
        displaced: Vec::new(),
    };
    if cancel.load(Ordering::Relaxed) {
        outcome.failure = Some(Failure {
            index: 0,
            reason: "Workspace edit cancelled before execution".into(),
        });
        return outcome;
    }
    for (path, id) in &plan.parents {
        let result = (|| -> Result<()> {
            check_components(path)?;
            ensure!(
                directory_identity(path)? == *id,
                "Parent directory identity changed while awaiting approval: {}",
                path.display()
            );
            Ok(())
        })();
        if let Err(error) = result {
            let index = plan
                .steps
                .iter()
                .find(|s| s.parents.iter().any(|(p, _)| p == path))
                .map_or(0, |s| s.index);
            outcome.failure = Some(Failure {
                index,
                reason: format!("{error:#}"),
            });
            return outcome;
        }
    }
    for (path, state) in &plan.initial {
        let result = (|| -> Result<()> {
            ensure!(
                matches_state(state, &scan_cancellable(path, &mut 0, cancel)?),
                "Filesystem changed while awaiting approval: {}",
                path.display()
            );
            Ok(())
        })();
        if let Err(error) = result {
            let index = plan
                .steps
                .iter()
                .find(|s| s.expected.iter().any(|(p, _)| p == path))
                .map_or(0, |s| s.index);
            outcome.failure = Some(Failure {
                index,
                reason: format!("{error:#}"),
            });
            return outcome;
        }
    }
    let mut created = BTreeMap::new();
    for step in plan.steps {
        if cancel.load(Ordering::Relaxed) {
            outcome.failure = Some(Failure {
                index: step.index,
                reason: "Workspace edit cancelled; completed prefix retained".into(),
            });
            break;
        }
        let mut committed = false;
        let displaced_start = outcome.displaced.len();
        let result = (|| -> Result<BTreeMap<PathBuf, DiskState>> {
            for (path, id) in &step.parents {
                check_components(path)?;
                ensure!(
                    directory_identity(path)? == *id,
                    "Resource parent identity changed: {}",
                    path.display()
                );
            }
            let _guards = commit_guards(step.expected.iter().map(|(path, _)| path.clone()))?;
            for (path, state) in &step.expected {
                let moved = if let Some(baseline) = created.get(path) {
                    files::verify_state_cancellable(path, baseline, || {
                        cancel.load(Ordering::Relaxed)
                    })?;
                    true
                } else {
                    false
                };
                let actual = scan_cancellable(path, &mut 0, cancel)?;
                ensure!(
                    if moved {
                        same_object(state, &actual)
                    } else {
                        matches_state(state, &actual)
                    },
                    "Resource changed before change {}: {}",
                    step.index + 1,
                    path.display()
                );
            }
            let mut baselines = BTreeMap::new();
            match &step.operation {
                Operation::Text { path, .. } => {
                    baselines.insert(path.clone(), files::disk_state(path)?);
                }
                Operation::Create { path, .. } => {
                    writable(path)?;
                    if path.exists() {
                        backup(
                            path,
                            &step.expected[0].1,
                            &mut outcome.backups,
                            &mut outcome.displaced,
                            cancel,
                        )?;
                    }
                    files::verify_state(path, &DiskState::Missing)?;
                    let temporary = tempfile::NamedTempFile::new_in(
                        path.parent().context("No create parent")?,
                    )?;
                    temporary.as_file().sync_all()?;
                    temporary.persist_noclobber(path).map_err(|error| {
                        anyhow::anyhow!("Atomic create failed: {}", error.error)
                    })?;
                    committed = true;
                    let baseline = files::disk_state(path)?;
                    ensure!(
                        matches!(baseline, DiskState::Present { bytes: 0, .. }),
                        "Created file changed at commit boundary; retained at destination"
                    );
                    baselines.insert(path.clone(), baseline);
                }
                Operation::Rename {
                    from,
                    to,
                    case_only,
                    ..
                } => {
                    writable(from)?;
                    writable(to)?;
                    if *case_only {
                        backup(
                            from,
                            &step.expected[0].1,
                            &mut outcome.backups,
                            &mut outcome.displaced,
                            cancel,
                        )?;
                        let staging = outcome.backups.last().unwrap().clone();
                        rename_no_replace(&staging.join("original"), to)?;
                        outcome.displaced.pop();
                        if std::fs::remove_file(staging.join("SOURCE.txt")).is_ok()
                            && std::fs::remove_dir(&staging).is_ok()
                        {
                            outcome.backups.pop();
                        }
                    } else {
                        if to.exists() {
                            backup(
                                to,
                                &step.expected[1].1,
                                &mut outcome.backups,
                                &mut outcome.displaced,
                                cancel,
                            )?;
                        }
                        ensure!(
                            !to.exists(),
                            "Rename destination changed at commit boundary"
                        );
                        rename_no_replace(from, to)?;
                    }
                    committed = true;
                    let after = scan_cancellable(to, &mut 0, cancel)?;
                    ensure!(
                        same_object(&step.expected[0].1, &after),
                        "Renamed object changed at commit boundary; retained at destination"
                    );
                    collect_baselines(to, &after, &mut baselines);
                }
                Operation::Delete { path } => {
                    writable(path)?;
                    backup(
                        path,
                        &step.expected[0].1,
                        &mut outcome.backups,
                        &mut outcome.displaced,
                        cancel,
                    )?;
                    committed = true;
                }
                Operation::Ignore => (),
            }
            Ok(baselines)
        })();
        match result {
            Ok(baselines) => {
                match &step.operation {
                    Operation::Rename { from, to, .. } => remap(&mut created, from, to),
                    Operation::Delete { path } => created.retain(|p, _| !p.starts_with(path)),
                    _ => (),
                }
                created.extend(baselines.clone());
                let completed = step.index + 1;
                outcome.applied.push(Applied {
                    step,
                    baselines,
                    displaced: outcome.displaced[displaced_start..].to_vec(),
                });
                progress(completed, total);
            }
            Err(error) => {
                outcome.failure = Some(Failure {
                    index: step.index,
                    reason: format!("{error:#}"),
                });
                if committed || outcome.displaced.len() > displaced_start {
                    let mut step = step;
                    if !committed {
                        step.operation = Operation::Ignore;
                    }
                    outcome.applied.push(Applied {
                        step,
                        baselines: BTreeMap::new(),
                        displaced: outcome.displaced[displaced_start..].to_vec(),
                    });
                }
                break;
            }
        }
    }
    outcome
}

#[cfg(all(test, windows))]
mod sharing_tests {
    use super::*;
    #[test]
    fn commit_guards_deny_parent_replacement_and_file_writes_without_blocking_child_creation() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let file = parent.join("file");
        std::fs::write(&file, "original").unwrap();
        let guards = commit_guards(std::iter::once(file.clone())).unwrap();
        assert!(std::fs::rename(&parent, root.path().join("replaced")).is_err());
        assert!(std::fs::OpenOptions::new().write(true).open(&file).is_err());
        std::fs::write(parent.join("unrelated"), "allowed").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
        drop(guards);
        std::fs::rename(&parent, root.path().join("replaced")).unwrap();
    }
}
