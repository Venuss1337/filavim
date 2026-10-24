//! Native launch arguments, resolved before terminal initialization.
//! Paths always use the launch directory, never an implicit process-wide chdir.
use crate::{
    app::App,
    buffer::Buffer,
    config::Config,
    editor::Editor,
    files,
    storage::History,
    workspace::{Workspace, find_project_root},
};
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub const HELP: &str = "FILAVIM — a quiet place to build something.

Usage: filavim [OPTIONS] [PATH ...]

Arguments:
  PATH           Files to open, and optionally one project directory
                 Relative paths use the directory you launch from
                 Missing files open as empty buffers; saving creates them

Options:
  -h, --help     Print this help without entering the editor
  -V, --version  Print the version without entering the editor
  --             Treat remaining arguments as paths, including leading dashes

Examples:
  filavim                    Dashboard in the current directory
  filavim .                  Open this project and its file explorer
  filavim ./my-project       Open a project directory
  filavim src/main.rs         Open a file
  filavim first.rs second.rs  Open buffers, focusing the first

Normal mode: W/A/S/D move, I inserts, Space opens leader commands.
Space f f finds files; Space e toggles the explorer; F1 shows controls.
FILAVIM_CONFIG overrides the TOML configuration path.";

#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    Help,
    Version,
    Open(Vec<PathBuf>),
}
impl Cli {
    /// Takes arguments after argv[0], preserving native OS filenames.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut paths = Vec::new();
        let mut positional = false;
        for arg in args {
            if !positional {
                if arg == "--" {
                    positional = true;
                    continue;
                }
                if arg == "--help" || arg == "-h" {
                    return Ok(Self::Help);
                }
                if arg == "--version" || arg == "-V" {
                    return Ok(Self::Version);
                }
                if arg.to_string_lossy().starts_with('-') {
                    bail!(
                        "Unknown option: {}. Use --help, or -- before a filename starting with '-'.",
                        arg.to_string_lossy()
                    );
                }
            }
            if arg.is_empty() {
                bail!("Empty path argument. Use . to open the current directory.");
            }
            paths.push(PathBuf::from(arg));
        }
        Ok(Self::Open(paths))
    }
}

pub fn version() -> &'static str {
    concat!("filavim ", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug)]
pub struct Launch {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub explicit_directory: bool,
}
impl Launch {
    pub fn resolve(cwd: &Path, paths: &[PathBuf]) -> Result<Self> {
        let cwd = directory(cwd)?;
        let mut explicit: Option<PathBuf> = None;
        let mut files = Vec::new();
        for arg in paths {
            if arg.as_os_str().is_empty() {
                bail!("Empty path argument");
            }
            #[cfg(windows)]
            if arg
                .components()
                .next()
                .is_some_and(|c| matches!(c, std::path::Component::Prefix(_)))
                && !arg.has_root()
            {
                bail!(
                    "Drive-relative paths are ambiguous: {}. Use an absolute drive path instead.",
                    arg.display()
                );
            }
            let path = cwd.join(arg);
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    let root = directory(&path)?;
                    if let Some(prior) = &explicit
                        && prior != &root
                    {
                        bail!(
                            "Conflicting project directories: {} and {}. Choose one directory; multiple files are supported.",
                            prior.display(),
                            root.display()
                        );
                    }
                    explicit = Some(root);
                }
                Ok(meta) if meta.is_file() => {
                    let path = fs::canonicalize(&path)
                        .with_context(|| format!("Cannot resolve {}", path.display()))?;
                    if !files.contains(&path) {
                        files.push(path);
                    }
                }
                Ok(_) => bail!("Not a regular file or directory: {}", path.display()),
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    if fs::symlink_metadata(&path).is_ok() {
                        bail!("Cannot resolve symbolic link: {}", path.display());
                    }
                    if arg
                        .as_os_str()
                        .to_string_lossy()
                        .ends_with(std::path::is_separator)
                    {
                        bail!("Directory does not exist: {}", path.display());
                    }
                    let name = path.file_name().context("New file path must name a file")?;
                    let parent = directory(path.parent().context("File path has no parent")?)
                        .with_context(|| format!("Cannot open new file {}: its parent must be an accessible directory", path.display()))?;
                    let path = parent.join(name);
                    if !files.contains(&path) {
                        files.push(path);
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("Cannot access {}", path.display()));
                }
            }
        }
        let root = explicit.clone().unwrap_or_else(|| {
            files
                .first()
                .and_then(|p| p.parent())
                .and_then(find_project_root)
                .unwrap_or(cwd)
        });
        // Discovery can find a parent the user cannot list; fail before raw mode.
        let root = directory(&root)?;
        Ok(Self {
            root,
            files,
            explicit_directory: explicit.is_some(),
        })
    }

    pub fn workspace(&self, config: Config) -> Result<Workspace> {
        let mut editor = Editor::new(config);
        if let Some(path) = self.files.first() {
            editor.set_buffer(Buffer::new(files::open(path)?, Some(path.clone())));
        }
        let mut workspace = Workspace::new(editor, self.root.clone());
        for path in self.files.iter().skip(1) {
            workspace.insert_buffer(Buffer::new(files::open(path)?, Some(path.clone())));
        }
        Ok(workspace)
    }

    pub fn into_app(self, config: Config, history: History, history_path: PathBuf) -> Result<App> {
        let workspace = self.workspace(config)?;
        let mut app = App::with_history_path(workspace, history, history_path);
        app.explorer.visible = self.explicit_directory;
        app.explorer.focused = self.explicit_directory && self.files.is_empty();
        Ok(app)
    }
}

fn directory(path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(path)
        .with_context(|| format!("Cannot resolve directory {}", path.display()))?;
    if !fs::metadata(&root)?.is_dir() {
        bail!("Not a directory: {}", path.display());
    }
    // Check actual listing access rather than trusting metadata/exists().
    fs::read_dir(&root)
        .with_context(|| format!("Cannot read directory {}", path.display()))?
        .next()
        .transpose()
        .with_context(|| format!("Cannot list directory {}", path.display()))?;
    Ok(root)
}
