use std::path::{Path, PathBuf};
/// Make a path absolute without interpreting `..` across symlinks or missing
/// directories. File jobs resolve it through the filesystem off the UI thread.
pub fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}
pub fn project_root(start: &Path) -> PathBuf {
    find_project_root(start).unwrap_or_else(|| {
        if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        }
    })
}
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    for dir in start.ancestors() {
        if [
            ".git",
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "go.mod",
            ".filavim",
        ]
        .iter()
        .any(|marker| dir.join(marker).exists())
        {
            return Some(dir.to_path_buf());
        }
    }
    None
}
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    }
}
