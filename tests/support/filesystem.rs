//! Small real-filesystem fixture. Every path is confined to an owned TempDir.
use std::{
    fs,
    path::{Path, PathBuf},
};
pub struct Workspace {
    directory: tempfile::TempDir,
}
impl Workspace {
    pub fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }
    pub fn root(&self) -> &Path {
        self.directory.path()
    }
    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root().join(name);
        // Fixture names are relative and cannot escape their owned root.
        assert!(
            Path::new(name)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}
