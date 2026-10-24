//! File operations run on a worker. Saves use a same-directory atomic replacement.
use anyhow::{Context, Result, bail};
use ropey::Rope;
use std::{
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
};

pub fn open(path: &Path) -> Result<Rope> {
    match File::open(path) {
        Ok(file) => {
            if !file.metadata()?.is_file() {
                bail!("{} is not a regular file", path.display());
            }
            Rope::from_reader(BufReader::new(file))
                .with_context(|| format!("Cannot read {} as UTF-8", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = parent(path);
            if !parent.is_dir() {
                bail!("Parent directory does not exist: {}", parent.display());
            }
            Ok(Rope::new())
        }
        Err(e) => Err(e).with_context(|| format!("Cannot open {}", path.display())),
    }
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
pub fn save(path: &Path, text: &Rope) -> Result<()> {
    // Follow an existing symlink rather than replacing the symlink itself.
    let target: PathBuf = if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::canonicalize(path).with_context(|| format!("Cannot resolve {}", path.display()))?
    } else {
        path.into()
    };
    let metadata = match fs::metadata(&target) {
        Ok(m) => {
            if !m.is_file() {
                bail!("{} is not a regular file", path.display());
            }
            if m.permissions().readonly() {
                bail!("{} is read-only", path.display());
            }
            Some(m)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut temp = tempfile::NamedTempFile::new_in(parent(&target))
        .context("Cannot create save file in destination directory")?;
    text.write_to(temp.as_file_mut())?;
    temp.flush()?;
    if let Some(metadata) = metadata {
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    temp.as_file().sync_all()?;
    temp.persist(&target)
        .map_err(|e| e.error)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

/// Streaming literal search, bounded by pattern size rather than document size.
pub fn find(text: &Rope, query: &str, start: usize) -> Option<usize> {
    let pattern: Vec<char> = query.chars().collect();
    if pattern.is_empty() {
        return None;
    }
    let mut failure = vec![0; pattern.len()];
    for i in 1..pattern.len() {
        let mut j = failure[i - 1];
        while j > 0 && pattern[i] != pattern[j] {
            j = failure[j - 1];
        }
        if pattern[i] == pattern[j] {
            j += 1;
        }
        failure[i] = j;
    }
    let scan = |from: usize, to: usize| {
        let mut matched = 0;
        for (offset, c) in text.slice(from..to).chars().enumerate() {
            while matched > 0 && c != pattern[matched] {
                matched = failure[matched - 1];
            }
            if c == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                return Some(from + offset + 1 - matched);
            }
        }
        None
    };
    let start = start.min(text.len_chars());
    scan(start, text.len_chars()).or_else(|| {
        scan(
            0,
            (start + pattern.len().saturating_sub(1)).min(text.len_chars()),
        )
        .filter(|p| *p < start)
    })
}
