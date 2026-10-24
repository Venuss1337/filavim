//! Opt-in font setup. No work is performed until an explicit setup/status task.
//! The provider is separate from installation: replace its bytes and identity
//! with a licensed redistributable asset without changing platform operations.
mod jsonc;
mod platform;
pub mod terminal;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const LABEL: &str = "Operator Mono Medium Nerd Font";
const FILE: &str = "filavim-operator-mono-medium-nerd.otf";
const STATE: &str = "operator-font-v1.json";
const MAX_FONT: u64 = 32 * 1024 * 1024;
#[cfg(feature = "dev-operator-font")]
pub fn bundled() -> Option<&'static [u8]> {
    Some(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.local/fonts/Operator Mono Medium Nerd Font Complete.otf"
    )))
}
#[cfg(not(feature = "dev-operator-font"))]
pub fn bundled() -> Option<&'static [u8]> {
    None
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Metadata {
    pub family: String,
    pub legacy_family: String,
    pub style: String,
    pub full_name: String,
    pub postscript: String,
    pub weight: u16,
    /// Representative Nerd Font symbols, not proof of host-terminal rendering.
    pub nerd_glyphs: bool,
}
pub fn metadata(bytes: &[u8]) -> Result<Metadata> {
    ensure!(
        bytes.len() as u64 <= MAX_FONT,
        "Font exceeds the 32 MiB safety limit"
    );
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|e| anyhow::anyhow!("Invalid OpenType font: {e:?}"))?;
    ensure!(face.number_of_glyphs() > 0, "Font has no glyphs");
    ensure!(
        face.tables().glyf.is_some() || face.tables().cff.is_some(),
        "Font has no valid TrueType or CFF outlines"
    );
    let name = |id| {
        face.names()
            .into_iter()
            .filter(|n| n.name_id == id)
            .find_map(|n| n.to_string())
    };
    let legacy_family =
        name(ttf_parser::name_id::FAMILY).context("Font has no Unicode family name")?;
    let family =
        name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).unwrap_or_else(|| legacy_family.clone());
    ensure!(
        !family.is_empty() && !family.chars().any(char::is_control),
        "Invalid font family"
    );
    Ok(Metadata {
        family,
        legacy_family,
        style: name(ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
            .or_else(|| name(ttf_parser::name_id::SUBFAMILY))
            .context("Font has no style")?,
        full_name: name(ttf_parser::name_id::FULL_NAME).context("Font has no full name")?,
        postscript: name(ttf_parser::name_id::POST_SCRIPT_NAME)
            .context("Font has no PostScript name")?,
        weight: face.weight().to_number(),
        nerd_glyphs: ['\u{e0b0}', '\u{e5ff}', '\u{f120}', '\u{f07b}']
            .iter()
            .all(|c| face.glyph_index(*c).is_some()),
    })
}
/// Verified reference metadata lets ordinary builds detect an existing install.
/// This is text only; ordinary builds contain no proprietary font data.
fn reference() -> Metadata {
    Metadata {
        family: "OperatorMono Nerd Font".into(),
        legacy_family: "Operator Mono Medium".into(),
        style: "Medium".into(),
        full_name: "Operator Mono Medium Nerd Font Complete".into(),
        postscript: "OperatorMonoNerdFontComplete - Medium".into(),
        weight: 350,
        nerd_glyphs: false,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOS,
}
impl Platform {
    pub fn current() -> Result<Self> {
        if cfg!(windows) {
            Ok(Self::Windows)
        } else if cfg!(target_os = "macos") {
            Ok(Self::MacOS)
        } else if cfg!(target_os = "linux") {
            Ok(Self::Linux)
        } else {
            bail!("Font installation is unsupported on this operating system")
        }
    }
}
#[derive(Clone, Default)]
pub struct Environment(pub BTreeMap<String, String>);
impl Environment {
    pub fn current() -> Self {
        Self(
            std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
        )
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str).filter(|v| !v.is_empty())
    }
    pub fn path(&self, k: &str) -> Option<PathBuf> {
        self.get(k).map(PathBuf::from).filter(|p| p.is_absolute())
    }
}
#[derive(Clone, Debug)]
pub struct Paths {
    pub fonts: PathBuf,
    pub state: PathBuf,
    pub search: Vec<PathBuf>,
}
pub fn paths(platform: Platform, env: &Environment) -> Result<Paths> {
    let home = || {
        env.path(if platform == Platform::Windows {
            "USERPROFILE"
        } else {
            "HOME"
        })
        .context("No absolute home directory is available")
    };
    let (fonts, state, search) = match platform {
        Platform::Windows => {
            let local = env
                .path("LOCALAPPDATA")
                .context("LOCALAPPDATA is missing or not absolute")?;
            let fonts = local.join("Microsoft/Windows/Fonts");
            let mut search = vec![fonts.clone()];
            if let Some(windows) = env.path("WINDIR") {
                search.push(windows.join("Fonts"));
            }
            (fonts, local.join("filavim/font-setup"), search)
        }
        Platform::Linux => {
            let home = home()?;
            let data = env
                .path("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"));
            let fonts = data.join("fonts/filavim");
            let search = vec![
                data.join("fonts"),
                home.join(".fonts"),
                PathBuf::from("/usr/local/share/fonts"),
                PathBuf::from("/usr/share/fonts"),
            ];
            (
                fonts,
                env.path("XDG_STATE_HOME")
                    .unwrap_or_else(|| home.join(".local/state"))
                    .join("filavim/font-setup"),
                search,
            )
        }
        Platform::MacOS => {
            let home = home()?;
            let fonts = home.join("Library/Fonts");
            (
                fonts.clone(),
                home.join("Library/Application Support/filavim/font-setup"),
                vec![
                    fonts,
                    PathBuf::from("/Library/Fonts"),
                    PathBuf::from("/System/Library/Fonts"),
                ],
            )
        }
    };
    Ok(Paths {
        fonts,
        state,
        search,
    })
}
#[derive(Serialize, Deserialize)]
struct Managed {
    version: u8,
    hash: String,
    metadata: Metadata,
}
#[derive(Serialize, Deserialize)]
struct ConfigChange {
    path: PathBuf,
    scope: String,
    backup: PathBuf,
    before_hash: String,
    after_hash: String,
    family: String,
}
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn read_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = fs::File::open(path)
        .with_context(|| format!("Cannot read {} (check permissions)", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} exceeds the {limit}-byte safety limit",
        path.display()
    );
    Ok(bytes)
}
pub(super) fn read_limited(path: &Path, limit: u64) -> Result<String> {
    String::from_utf8(read_bytes(path, limit)?).context("Configuration is not UTF-8")
}
fn regular(path: &Path) -> Result<()> {
    let m =
        fs::symlink_metadata(path).with_context(|| format!("Cannot inspect {}", path.display()))?;
    ensure!(
        m.is_file() && !m.file_type().is_symlink(),
        "Refusing a non-regular file: {}",
        path.display()
    );
    ensure!(
        !m.permissions().readonly(),
        "{} is read-only; check file permissions",
        path.display()
    );
    Ok(())
}
fn exists_regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
        Ok(_) => {
            regular(path)?;
            Ok(true)
        }
    }
}
fn atomic(path: &Path, bytes: &[u8], new: bool) -> Result<()> {
    let parent = path
        .parent()
        .context("Destination has no parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "Cannot create a temporary file in {}; check permissions",
            parent.display()
        )
    })?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    if !new {
        regular(path)?;
        temp.as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    let result = if new {
        temp.persist_noclobber(path)
    } else {
        temp.persist(path)
    };
    result.map_err(|e| e.error).with_context(|| {
        format!(
            "Cannot safely write {}; check permissions or locked files",
            path.display()
        )
    })?;
    Ok(())
}
fn load(paths: &Paths) -> Result<Option<Managed>> {
    let p = paths.state.join(STATE);
    match fs::symlink_metadata(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(m) => {
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "Invalid font ownership record"
            );
            let record: Managed = serde_json::from_slice(&read_bytes(&p, 64 * 1024)?)?;
            ensure!(record.version == 1, "Unsupported font ownership record");
            Ok(Some(record))
        }
    }
}
/// Platform hooks are injected for isolated tests. Production never shells out
/// for Windows registration and never interpolates paths into shell scripts.
pub trait Registration {
    fn existing(&self, _metadata: &Metadata) -> Result<Option<PathBuf>> {
        Ok(None)
    }
    fn register(&self, path: &Path, metadata: &Metadata) -> Result<String>;
    fn unregister(&self, path: &Path, metadata: &Metadata) -> Result<String>;
    fn removed(&self, _directory: &Path) -> Result<String> {
        Ok(String::new())
    }
}
pub fn install(paths: &Paths, bytes: &[u8], registration: &impl Registration) -> Result<String> {
    let m = metadata(bytes)?;
    let target = paths.fonts.join(FILE);
    if let Some(record) = load(paths)? {
        ensure!(
            record.hash == hash(bytes),
            "A different managed font is recorded. Remove it explicitly before installing this version."
        );
        let existed = exists_regular(&target)?;
        if existed {
            ensure!(
                hash(&read_bytes(&target, MAX_FONT)?) == record.hash,
                "Managed font was modified; refusing to overwrite it"
            );
        } else {
            fs::create_dir_all(&paths.fonts)?;
            atomic(&target, bytes, true)?;
        }
        let note = registration.register(&target, &m)?;
        return Ok(format!(
            "{} {note}",
            if existed {
                "Font already installed; no font file was rewritten."
            } else {
                "Repaired a missing managed font file; ownership retained."
            }
        ));
    }
    let existing = match registration.existing(&m)? {
        Some(path) => Some(path),
        None => find_installed(paths, &m)?.map(|(p, _)| p),
    };
    if let Some(path) = existing {
        return Ok(format!(
            "Font already present at {}. This pre-existing installation is not owned or removable by Filavim.",
            path.display()
        ));
    }
    fs::create_dir_all(&paths.fonts)
        .context("Cannot create per-user font directory; check permissions")?;
    fs::create_dir_all(&paths.state)
        .context("Cannot create font ownership directory; check permissions")?;
    // Reserve ownership before registration. A crash leaves a recoverable managed
    // installation; a pre-existing destination is never overwritten or adopted.
    ensure!(
        !target.try_exists()?,
        "Destination already exists and is not managed: {}",
        target.display()
    );
    atomic(&target, bytes, true)?;
    let record = Managed {
        version: 1,
        hash: hash(bytes),
        metadata: m.clone(),
    };
    if let Err(e) = atomic(
        &paths.state.join(STATE),
        &serde_json::to_vec_pretty(&record)?,
        true,
    ) {
        let rollback = fs::remove_file(&target);
        bail!("Cannot record font ownership: {e:#}; file rollback: {rollback:?}");
    }
    match registration.register(&target, &m) {
        Ok(note) => Ok(format!(
            "Installed {} for the current user at {}. {note}\nTerminal configuration is a separate action.",
            m.family,
            target.display()
        )),
        Err(e) => {
            // Keep the ownership record on any incomplete rollback for repair.
            let rollback = registration
                .unregister(&target, &m)
                .and_then(|_| fs::remove_file(&target).map_err(Into::into));
            if rollback.is_ok() {
                fs::remove_file(paths.state.join(STATE))?;
            }
            bail!(
                "Font registration failed: {e:#}. Rollback: {rollback:?}. Retry setup or :RemoveFont; no unrelated installation was overwritten."
            )
        }
    }
}
pub fn remove(paths: &Paths, registration: &impl Registration) -> Result<String> {
    let record = load(paths)?.context(
        "No Filavim-managed font exists. Pre-existing installations will not be removed.",
    )?;
    let target = paths.fonts.join(FILE);
    let existed = exists_regular(&target)?;
    if existed {
        ensure!(
            hash(&read_bytes(&target, MAX_FONT)?) == record.hash,
            "Font file changed since installation; refusing removal"
        );
    }
    let note = registration.unregister(&target, &record.metadata)?;
    if existed && let Err(e) = fs::remove_file(&target) {
        let repair = registration.register(&target, &record.metadata);
        bail!(
            "Cannot remove {}: {e}. Close applications using the font and retry. Registration repair: {repair:?}",
            target.display()
        );
    }
    fs::remove_file(paths.state.join(STATE))?;
    let refresh = registration.removed(&paths.fonts).unwrap_or_else(|e| {
        format!("Font removed, but cache refresh failed: {e:#}; restart the terminal.")
    });
    Ok(format!(
        "Removed the Filavim-managed font. {note} {refresh}\nTerminal settings and backups were preserved. Select another terminal font if needed; user changes are never blindly restored."
    ))
}
fn find_installed(paths: &Paths, wanted: &Metadata) -> Result<Option<(PathBuf, Metadata)>> {
    let mut pending: Vec<_> = paths.search.iter().map(|p| (p.clone(), 0)).collect();
    let mut visited = 0;
    while let Some((dir, depth)) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Cannot inspect font directory {}", dir.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() && depth < 8 {
                pending.push((path, depth + 1));
                continue;
            }
            if !kind.is_file()
                || !path
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("otf") || s.eq_ignore_ascii_case("ttf"))
            {
                continue;
            }
            visited += 1;
            ensure!(
                visited <= 20000,
                "Font discovery exceeded 20,000 files; installation status is unknown"
            );
            // Invalid unrelated fonts do not prevent discovery. Access errors do.
            if entry.metadata()?.len() > MAX_FONT {
                continue;
            }
            let bytes = read_bytes(&path, MAX_FONT)?;
            if let Ok(m) = metadata(&bytes)
                && m.postscript == wanted.postscript
            {
                return Ok(Some((path, m)));
            }
        }
    }
    Ok(None)
}
#[derive(Clone, Debug)]
pub enum Operation {
    Install,
    Configure(terminal::Target),
    Both(terminal::Target),
    Remove,
}
pub struct Report {
    pub summary: String,
    pub targets: Vec<terminal::Target>,
    pub available: bool,
    pub managed: bool,
    pub installed: bool,
}
pub fn status() -> Result<Report> {
    let env = Environment::current();
    let platform = Platform::current()?;
    let paths = paths(platform, &env)?;
    let available = bundled().is_some();
    let wanted = match bundled() {
        Some(b) => metadata(b)?,
        None => reference(),
    };
    let record = load(&paths)?;
    let mut found = find_installed(&paths, &wanted)?;
    if found.is_none()
        && record.is_none()
        && let Some(path) = platform::Native(platform).existing(&wanted)?
    {
        let metadata = metadata(&read_bytes(&path, MAX_FONT)?)?;
        found = Some((path, metadata));
    }
    let installed = found.is_some();
    let managed = match record.as_ref() {
        Some(r) => {
            !exists_regular(&paths.fonts.join(FILE))?
                || read_bytes(&paths.fonts.join(FILE), MAX_FONT).is_ok_and(|b| hash(&b) == r.hash)
        }
        None => false,
    };
    let terminal = terminal::inspect(&env, platform);
    let glyphs = found
        .as_ref()
        .map(|(_, m)| m)
        .or_else(|| available.then_some(&wanted))
        .map_or("unknown", |m| {
            if m.nerd_glyphs {
                "sample cmap coverage verified (E0B0, E5FF, F120, F07B); host rendering unverified"
            } else {
                "sample incomplete; no claim of full Nerd Font coverage"
            }
        });
    let current = if terminal.targets.len() == 1 {
        terminal.targets[0]
            .current
            .as_deref()
            .unwrap_or("not explicitly configured; terminal default or runtime override unknown")
    } else {
        "unknown; select settings file and scope explicitly"
    };
    let summary = format!(
        "Font: {LABEL}\nFamily: {}\nLegacy family: {}\nStyle: {}; weight: {}\nPostScript: {}\nDevelopment bundle: {}\nInstallation: {}\nManaged by Filavim: {}\nOS: {platform:?}\nTerminal: {:?}\nConfigured font: {current}\nAutomatic configuration: {}\n{}\nNerd glyphs: {glyphs}\n\nChanges require an explicit action: install one per-user OTF and ownership record; on Windows register it in HKCU and notify applications. Configuration changes only font.face in the chosen Windows Terminal scope, with a backup; font size is preserved. Installation alone does not change terminal rendering.\n\n{}\n\nPrivate development asset; embedding or GitHub availability does not grant redistribution rights. Do not distribute this development executable.",
        wanted.family,
        wanted.legacy_family,
        wanted.style,
        wanted.weight,
        wanted.postscript,
        if available {
            "available (private development feature)"
        } else {
            "unavailable; build locally with dev-operator-font"
        },
        found
            .as_ref()
            .map(|(p, _)| p.display().to_string())
            .unwrap_or_else(|| "not found in scanned font directories".into()),
        managed,
        terminal.kind,
        if terminal.targets.is_empty() {
            "manual only"
        } else {
            "Windows Terminal; choose the path and scope"
        },
        terminal.note,
        terminal::manual(&terminal.kind, &wanted.family)
    );
    Ok(Report {
        summary,
        targets: terminal.targets,
        available,
        managed,
        installed,
    })
}
pub fn execute(operation: Operation) -> Result<String> {
    let env = Environment::current();
    let platform = Platform::current()?;
    let paths = paths(platform, &env)?;
    let registration = platform::Native(platform);
    match operation {
        Operation::Install => install(
            &paths,
            bundled().context("Development font is not bundled; see FONT_SETUP.md")?,
            &registration,
        ),
        Operation::Remove => remove(&paths, &registration),
        Operation::Configure(target) => configure(&env, &paths, &target),
        Operation::Both(target) => {
            let installed = install(
                &paths,
                bundled().context("Development font is not bundled")?,
                &registration,
            )?;
            match configure(&env, &paths, &target) {
                Ok(s) => Ok(format!("{installed}\n\n{s}")),
                Err(e) => Ok(format!(
                    "{installed}\n\nTerminal configuration FAILED: {e:#}. Font installation succeeded; configure manually or retry."
                )),
            }
        }
    }
}
fn configure(env: &Environment, paths: &Paths, target: &terminal::Target) -> Result<String> {
    terminal::validate_target(env, &target.path)?;
    let wanted = match bundled() {
        Some(b) => metadata(b)?,
        None => reference(),
    };
    ensure!(
        find_installed(paths, &wanted)?.is_some()
            || platform::Native(Platform::current()?)
                .existing(&wanted)?
                .is_some(),
        "Install the font before configuring the terminal"
    );
    configure_file(paths, &target.path, &target.scope, &wanted.family)
}
/// Compare-before-write, durable backup, atomic replacement, verify, and rollback.
/// The change journal is for inspection only: removal never replays it blindly.
fn configure_file(
    paths: &Paths,
    path: &Path,
    scope: &terminal::Scope,
    family: &str,
) -> Result<String> {
    configure_with(paths, path, scope, family, &DiskSettings)
}
trait SettingsIo {
    fn read(&self, path: &Path) -> Result<String>;
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()>;
}
struct DiskSettings;
impl SettingsIo for DiskSettings {
    fn read(&self, path: &Path) -> Result<String> {
        read_limited(path, 8 * 1024 * 1024)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        atomic(path, bytes, false)
    }
}
fn configure_with(
    paths: &Paths,
    path: &Path,
    scope: &terminal::Scope,
    family: &str,
    io: &impl SettingsIo,
) -> Result<String> {
    regular(path)?;
    let before = io.read(path)?;
    if terminal::configured(&before, scope)?.as_deref() == Some(family) {
        return Ok("Terminal scope already uses the requested font; no settings rewritten. Runtime rendering cannot be queried.".into());
    }
    let after = terminal::edit(&before, scope, family)?;
    fs::create_dir_all(&paths.state)?;
    let mut backup = tempfile::Builder::new()
        .prefix("settings-")
        .suffix(".jsonc.bak")
        .tempfile_in(&paths.state)?;
    backup.write_all(before.as_bytes())?;
    backup.as_file().sync_all()?;
    let (_, backup) = backup.keep().map_err(|e| e.error)?;
    let change = ConfigChange {
        path: path.into(),
        scope: format!("{scope:?}"),
        backup: backup.clone(),
        before_hash: hash(before.as_bytes()),
        after_hash: hash(after.as_bytes()),
        family: family.into(),
    };
    let journal = paths.state.join(format!(
        "change-{}.json",
        hash(backup.to_string_lossy().as_bytes())
    ));
    atomic(&journal, &serde_json::to_vec_pretty(&change)?, true)?;
    ensure!(
        io.read(path)? == before,
        "Terminal settings changed while preparing the edit; nothing was overwritten. Retry setup."
    );
    if let Err(error) = io.write(path, after.as_bytes()) {
        // A publisher can fail after replacement (e.g. a durability error).
        // Roll back only when the destination still contains our exact bytes.
        if io.read(path).is_ok_and(|actual| actual == after) {
            io.write(path, before.as_bytes()).with_context(|| {
                format!(
                    "Write failed: {error:#}; rollback failed. Backup: {}",
                    backup.display()
                )
            })?;
            ensure!(
                io.read(path)? == before,
                "Rollback could not be verified; backup: {}",
                backup.display()
            );
            bail!(
                "Configuration write failed: {error:#}; original settings restored. Backup: {}",
                backup.display()
            );
        }
        bail!(
            "Configuration write failed: {error:#}. No blind rollback was attempted; original backup: {}",
            backup.display()
        );
    }
    let actual = io.read(path).with_context(|| {
        format!(
            "Cannot verify terminal settings. No blind rollback was attempted; backup: {}",
            backup.display()
        )
    })?;
    ensure!(
        actual == after,
        "Terminal settings changed immediately after writing. Refusing automatic rollback over a concurrent edit. Original backup: {}",
        backup.display()
    );
    ensure!(
        terminal::configured(&actual, scope)?.as_deref() == Some(family),
        "Font configuration verification failed; backup: {}",
        backup.display()
    );
    Ok(format!(
        "Windows Terminal configuration written and verified: {}\nScope: {scope:?}\nFont face: {family}\nBackup: {}\nFont size and unrelated settings were preserved. Reopen the tab or restart Terminal if the newly installed font is not available. Runtime font rendering is not directly observable.",
        path.display(),
        backup.display()
    ))
}

#[cfg(test)]
mod tests;
