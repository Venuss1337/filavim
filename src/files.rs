//! Synchronous UTF-8 file operations and literal Rope search.
//!
//! Callers must schedule blocking disk work off the UI thread (see
//! [`crate::gui::io`]). Saves use a same-directory temporary file and atomic
//! replacement; a missing file opens as empty only when its parent exists.
use anyhow::{Context, Result, bail};
use ropey::Rope;
use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
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
/// Persist a snapshot via a synced temporary file beside the destination.
/// Existing symlinks are followed, permissions retained, and read-only targets
/// rejected. This blocking operation does not update Buffer's saved-state identity.
pub fn save(path: &Path, text: &Rope) -> Result<()> {
    save_impl(path, text, None)
}
fn save_impl(path: &Path, text: &Rope, expected: Option<&DiskState>) -> Result<()> {
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
    let creating = metadata.is_none();
    if let Some(metadata) = metadata {
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    temp.as_file().sync_all()?;
    // On Windows this denies ordinary write handles during final verification
    // and replacement, but permits deletion so our atomic rename can succeed.
    // A path replacement by another process can still race; this is not CAS.
    let _guard = if expected.is_some() {
        save_guard(path)?
    } else {
        None
    };
    if let Some(expected) = expected {
        verify_state(path, expected)?;
    }
    if creating || matches!(expected, Some(DiskState::Missing)) {
        // Creation has a true no-clobber primitive: another process winning the
        // final check-to-create race must retain its file.
        match temp.persist_noclobber(&target) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Conflict {
                    actual: disk_state(path)?,
                }
                .into());
            }
            Err(error) => return Err(error.error).context("Cannot create destination"),
        }
    } else {
        replace_temp(temp, &target, expected)
            .with_context(|| format!("Cannot replace {}", path.display()))?;
    }
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

/// Content verification is intentional even when metadata is unchanged: tools can
/// preserve timestamps. This is called only by the serialized I/O worker.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiskState {
    Missing,
    Present {
        #[serde(default)]
        identity: Option<[u64; 2]>,
        #[serde(default)]
        bytes: u64,
        digest: [u8; 32],
        modified: Option<std::time::SystemTime>,
        created: Option<std::time::SystemTime>,
    },
}
pub fn disk_state(path: &Path) -> Result<DiskState> {disk_state_cancellable(path,||false)}
pub(crate) fn disk_state_cancellable(path:&Path,cancel:impl Fn()->bool)->Result<DiskState>{
    use sha2::{Digest, Sha256};
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DiskState::Missing),
        Err(e) => return Err(e).context("Cannot verify destination"),
    };
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("Destination is not a regular file");
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        if cancel(){bail!("Operation cancelled");}
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        bail!("File changed while being read; retry the operation");
    }
    Ok(DiskState::Present {
        identity: file_identity(&file)?,
        bytes: after.len(),
        digest: hash.finalize().into(),
        modified: after.modified().ok(),
        created: after.created().ok(),
    })
}
/// Resolve the parent through the filesystem, never by lexical removal of `..`.
pub fn resolve_path(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is not a new file.
            if fs::symlink_metadata(path).is_ok() {
                bail!("Cannot resolve destination symlink");
            }
            let name = path.file_name().context("Path has no filename")?;
            Ok(fs::canonicalize(parent(path))
                .context("Cannot resolve parent directory")?
                .join(name))
        }
        Err(e) => Err(e).context("Cannot resolve file path"),
    }
}
pub fn open_tracked(path: &Path) -> Result<(Rope, DiskState)> {
    let before = disk_state(path)?;
    let text = open(path)?;
    let after = disk_state(path)?;
    if before != after {
        bail!("File changed while opening; retry");
    }
    Ok((text, after))
}
pub fn save_checked(path: &Path, text: &Rope, expected: &DiskState) -> Result<DiskState> {
    verify_state(path, expected)?;
    save_impl(path, text, Some(expected))?;
    let saved = disk_state(path)?;
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for chunk in text.chunks() {
        hash.update(chunk.as_bytes());
    }
    let digest: [u8; 32] = hash.finalize().into();
    if !matches!(&saved, DiskState::Present { digest: actual, .. } if *actual == digest) {
        bail!("Destination changed immediately after saving; editor contents remain unsaved");
    }
    Ok(saved)
}
#[derive(Debug)]
pub struct Conflict {
    pub actual: DiskState,
}
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "File changed, was replaced, or deleted outside FilaVim; editor changes are preserved",
        )
    }
}
impl std::error::Error for Conflict {}
pub(crate) fn verify_state(path: &Path, expected: &DiskState) -> Result<()> {verify_state_cancellable(path,expected,||false)}
pub(crate) fn verify_state_cancellable(path:&Path,expected:&DiskState,cancel:impl Fn()->bool)->Result<()> {
    let actual = disk_state_cancellable(path,cancel)?;
    if &actual != expected {
        return Err(Conflict { actual }.into());
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn file_identity(file: &File) -> Result<Option<[u64; 2]>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // The file owns this handle for the duration of the call; the API fills the
    // initialized output structure and does not retain either pointer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Some([
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ]))
}
#[cfg(unix)]
pub(crate) fn file_identity(file: &File) -> Result<Option<[u64; 2]>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Some([metadata.dev(), metadata.ino()]))
}
#[cfg(not(any(unix, windows)))]
pub(crate) fn file_identity(_: &File) -> Result<Option<[u64; 2]>> {
    Ok(None)
}

#[cfg(windows)]
fn replace_temp(
    temp: tempfile::NamedTempFile,
    target: &Path,
    expected: Option<&DiskState>,
) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let path = temp.into_temp_path();
    let backup = tempfile::Builder::new()
        .prefix(".filavim-backup-")
        .tempfile_in(parent(target))?
        .into_temp_path();
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let destination = wide(target);
    let source = wide(&path);
    let old = wide(&backup);
    // All pointers live through this synchronous call. READ/DELETE target access
    // is compatible with save_guard. A backup protects ReplaceFile's documented
    // partial-failure cases; it is discarded only after checking the displaced file.
    if unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            source.as_ptr(),
            old.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        let backup = backup.keep()?;
        let snapshot = path.keep()?;
        return Err(error).with_context(|| {
            format!(
                "Replacement failed; retained backup at {} and save snapshot at {} for inspection",
                backup.display(),
                snapshot.display()
            )
        });
    }
    if let Some(expected) = expected {
        let displaced = disk_state(&backup);
        if !displaced.as_ref().is_ok_and(|state| state == expected) {
            let backup = backup.keep()?;
            bail!(
                "Destination changed during replacement or its backup could not be verified. External version retained at {}. The editor snapshot is on disk, but remains unsaved in the editor; inspect both versions",
                backup.display()
            );
        }
    }
    backup
        .close()
        .context("Saved, but could not remove verified backup")?;
    Ok(())
}
#[cfg(not(windows))]
fn replace_temp(temp: tempfile::NamedTempFile, target: &Path, _: Option<&DiskState>) -> Result<()> {
    temp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn save_guard(path: &Path) -> Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
    match fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Cannot protect destination while saving; another program may hold write access. Close that handle and retry"),
    }
}
#[cfg(not(windows))]
pub(crate) fn save_guard(_: &Path) -> Result<Option<File>> {
    Ok(None)
}

#[cfg(all(test, windows))]
mod sharing_tests {
    use super::*;
    #[test]
    fn guard_denies_writers_but_allows_atomic_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "initial").unwrap();
        let guard = save_guard(&path).unwrap();
        assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
        let replacement = directory.path().join("replacement");
        fs::write(&replacement, "replaced").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "replaced");
        drop(guard);
    }

    #[test]
    fn replacement_boundary_conflict_preserves_the_displaced_external_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "initial").unwrap();
        let expected = disk_state(&path).unwrap();
        let mut temp = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        temp.write_all(b"editor").unwrap();
        temp.as_file().sync_all().unwrap();
        // Exercise the actual commit primitive after a change at its boundary.
        fs::write(&path, "external").unwrap();
        let error = replace_temp(temp, &path, Some(&expected)).unwrap_err();
        assert!(error.to_string().contains("External version retained"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "editor");
        let backups: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".filavim-backup-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "external");
    }
    #[test]
    fn failed_replace_retains_save_snapshot_and_original() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "original").unwrap();
        let expected = disk_state(&path).unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        let mut temp = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        temp.write_all(b"snapshot").unwrap();
        let retained = temp.path().to_path_buf();
        assert!(replace_temp(temp, &path, Some(&expected)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(fs::read_to_string(retained).unwrap(), "snapshot");
        drop(handle);
    }
    #[test]
    fn guarded_write_process_fixture() {
        let Some(path) = std::env::var_os("FILAVIM_GUARDED_FILE") else {
            return;
        };
        assert!(fs::OpenOptions::new().write(true).open(path).is_err());
    }
    #[test]
    fn guard_is_enforced_against_a_separate_process() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "original").unwrap();
        let guard = save_guard(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "files::sharing_tests::guarded_write_process_fixture",
                "--nocapture",
            ])
            .env("FILAVIM_GUARDED_FILE", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        drop(guard);
    }
    #[test]
    fn an_existing_writer_causes_a_safe_failed_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "initial").unwrap();
        let baseline = disk_state(&path).unwrap();
        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(save_checked(&path, &Rope::from_str("editor"), &baseline).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "initial");
        drop(writer);
        save_checked(&path, &Rope::from_str("editor"), &baseline).unwrap();
    }
}
