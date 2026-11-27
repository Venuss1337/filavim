//! Managed dependencies and bounded archive handling for the existing installer.
use super::*;
use sha2::{Digest, Sha256};

pub(super) fn trusted(url: &str) -> Result<()> {
    let url = url::Url::parse(url)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "Trusted HTTPS URL required"
    );
    let path = url.path();
    ensure!(
        match url.host_str().unwrap_or("") {
            "registry.npmjs.org" => true,
            "nodejs.org" => path.starts_with("/dist/"),
            "api.github.com" =>
                path.starts_with("/repos/rust-lang/rust-analyzer/releases/")
                    || path.starts_with("/repos/clangd/clangd/releases/")
                    || path.starts_with("/repos/astral-sh/ruff/releases/"),
            "github.com" =>
                path.starts_with("/rust-lang/rust-analyzer/releases/download/")
                    || path.starts_with("/clangd/clangd/releases/download/"),
            "release-assets.githubusercontent.com" => true,
            "pypi.org" => path.starts_with("/pypi/ruff/"),
            _ => false,
        },
        "Unexpected download host or path"
    );
    Ok(())
}
fn hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify(path: &Path, digest: &str) -> Result<()> {
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "Publisher SHA-256 unavailable or malformed"
    );
    ensure!(
        hash(path)?.eq_ignore_ascii_case(digest),
        "Artifact checksum mismatch"
    );
    Ok(())
}
fn relative(path: &Path) -> Result<()> {
    ensure!(
        path.components().all(|c| matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )),
        "Archive path escapes installation"
    );
    ensure!(
        !path.to_string_lossy().contains(['\\', ':']),
        "Archive path contains platform escape syntax"
    );
    Ok(())
}
fn extract(archive: &Path, directory: &Path, zip: bool, cancel: &AtomicBool) -> Result<()> {
    let mut total = 0u64;
    let mut count = 0;
    if zip {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
        for index in 0..archive.len() {
            check(cancel)?;
            count += 1;
            ensure!(count <= 30000, "Too many archive entries");
            let mut member = archive.by_index(index)?;
            let path = member.enclosed_name().context("Archive path traversal")?;
            relative(&path)?;
            ensure!(
                member.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
                "Archive symlink rejected"
            );
            let destination = directory.join(path);
            if member.is_dir() {
                std::fs::create_dir_all(destination)?;
                continue;
            }
            total = total
                .checked_add(member.size())
                .context("Archive size overflow")?;
            ensure!(
                total <= 1024 * 1024 * 1024 && member.size() <= 256 * 1024 * 1024,
                "Archive extraction limit exceeded"
            );
            std::fs::create_dir_all(destination.parent().unwrap())?;
            std::io::copy(
                &mut member,
                &mut std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)?,
            )?;
        }
    } else {
        let decoder = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries()? {
            check(cancel)?;
            let mut entry = entry?;
            count += 1;
            ensure!(count <= 30000, "Too many archive entries");
            let path = entry.path()?.into_owned();
            relative(&path)?;
            let kind = entry.header().entry_type();
            // Official Node archives contain npm convenience symlinks. Explicit
            // node/npm-cli launches need none of them; never materialize links.
            if kind.is_symlink() || kind.is_hard_link() {
                continue;
            }
            let destination = directory.join(path);
            if kind.is_dir() {
                std::fs::create_dir_all(destination)?;
                continue;
            }
            ensure!(kind.is_file(), "Special archive entry rejected");
            total = total
                .checked_add(entry.size())
                .context("Archive size overflow")?;
            ensure!(
                total <= 1024 * 1024 * 1024 && entry.size() <= 256 * 1024 * 1024,
                "Archive extraction limit exceeded"
            );
            std::fs::create_dir_all(destination.parent().unwrap())?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            std::io::copy(&mut entry, &mut file)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(
                    entry.header().mode()? & 0o755,
                ))?;
            }
        }
    }
    Ok(())
}
pub(super) fn find_executable(root: &Path, name: &str) -> Result<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            files += 1;
            ensure!(files <= 30000, "Installation layout exceeds limit");
            let kind = entry.file_type()?;
            ensure!(!kind.is_symlink(), "Installation contains a symlink");
            if kind.is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() == name {
                return Ok(entry.path());
            }
        }
    }
    bail!("Expected executable {name} absent from installation")
}
pub(super) fn install_clang(
    release: &Value,
    directory: &Path,
    cancel: &AtomicBool,
    report: &impl Fn(&str),
) -> Result<()> {
    let platform = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "mac",
        _ => bail!("Unsupported clangd platform"),
    };
    let version = release["tag_name"]
        .as_str()
        .context("Missing clangd version")?;
    let name = format!("clangd-{platform}-{version}.zip");
    let asset = release["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["name"] == name))
        .context("No compatible clangd release")?;
    let url = asset["browser_download_url"]
        .as_str()
        .context("Missing clangd URL")?;
    trusted(url)?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .context("No published clangd SHA-256; refusing installation")?;
    let archive = directory.join("download.zip");
    report(&format!("Downloading clangd {version}"));
    download_progress(url, &archive, cancel, 256 * 1024 * 1024, report)?;
    verify(&archive, digest)?;
    extract(&archive, directory, true, cancel)?;
    std::fs::remove_file(archive)?;
    let executable = find_executable(
        directory,
        if cfg!(windows) {
            "clangd.exe"
        } else {
            "clangd"
        },
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = executable;
    Ok(())
}
fn existing_node(data: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
    if let Ok((node, _)) = npm()
        && compatible(&node, cancel)
    {
        return Ok(node);
    }
    let parent = data.join("runtimes/node").canonicalize()?;
    let node: PathBuf = serde_json::from_slice(&std::fs::read(parent.join("current.json"))?)?;
    let node = node.canonicalize()?;
    let relative = node
        .strip_prefix(&parent)
        .context("Managed Node is outside its runtime directory")?;
    ensure!(
        relative.components().next().is_some_and(|part| part
            .as_os_str()
            .to_string_lossy()
            .starts_with("generation-")),
        "Invalid managed Node generation"
    );
    ensure!(
        compatible(&node, cancel),
        "Managed Node runtime is missing or incompatible"
    );
    Ok(node)
}
fn compatible(node: &Path, cancel: &AtomicBool) -> bool {
    let mut probe = command(node, &["--version".into()]);
    probe.env_remove("NODE_OPTIONS").env_remove("NODE_PATH");
    run(probe, cancel, 5).is_ok_and(|s| {
        let v: Vec<_> = s
            .trim()
            .trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect();
        v.len() == 3 && (v[0] > 22 || v[0] == 22 && (v[1], v[2]) >= (22, 2))
    })
}
fn npm_cli(node: &Path) -> Result<PathBuf> {
    let parent = node.parent().context("Node has no parent")?;
    [
        parent.join("node_modules/npm/bin/npm-cli.js"),
        parent.join("../lib/node_modules/npm/bin/npm-cli.js"),
        parent.join("../share/nodejs/npm/bin/npm-cli.js"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .context("Node distribution has no npm CLI")
}
pub(super) fn node(
    data: &Path,
    cancel: &AtomicBool,
    report: &impl Fn(&str),
) -> Result<(PathBuf, PathBuf)> {
    if let Ok(node) = existing_node(data, cancel)
        && let Ok(cli) = npm_cli(&node)
    {
        return Ok((node, cli));
    }
    check(cancel)?;
    download_node(data, cancel, report)
}

pub(super) fn download_node(
    data: &Path,
    cancel: &AtomicBool,
    report: &impl Fn(&str),
) -> Result<(PathBuf, PathBuf)> {
    report("Resolving managed Node 24 LTS runtime");
    let releases = json_url("https://nodejs.org/dist/index.json", cancel)?;
    let release = releases
        .as_array()
        .and_then(|v| {
            v.iter().find(|r| {
                r["version"]
                    .as_str()
                    .is_some_and(|v| v.starts_with("v24.") && !v.contains('-'))
                    && r["lts"].as_str().is_some()
            })
        })
        .context("No stable Node 24 LTS release")?;
    let version = release["version"].as_str().unwrap();
    ensure!(
        version
            .chars()
            .all(|c| c.is_ascii_digit() || ".v".contains(c)),
        "Invalid Node version"
    );
    let platform = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "darwin",
        "linux" => "linux",
        _ => bail!("Unsupported Node platform"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => bail!("Unsupported Node architecture"),
    };
    let zip = cfg!(windows);
    let filename = format!(
        "node-{version}-{platform}-{arch}.{}",
        if zip { "zip" } else { "tar.gz" }
    );
    let base = format!("https://nodejs.org/dist/{version}");
    let checksums = tempfile::NamedTempFile::new()?;
    download(
        &format!("{base}/SHASUMS256.txt"),
        checksums.path(),
        cancel,
        1024 * 1024,
    )?;
    let hashes = std::fs::read_to_string(checksums.path())?;
    let digest = hashes
        .lines()
        .find_map(|line| {
            let mut p = line.split_whitespace();
            let hash = p.next()?;
            (p.next()? == filename).then_some(hash)
        })
        .context("Node artifact missing from official checksum manifest")?;
    let parent = data.join("runtimes/node");
    std::fs::create_dir_all(&parent)?;
    let staging = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&parent)?;
    std::fs::write(staging.path().join(".filavim-owned"), "node")?;
    let archive = staging.path().join("download");
    download_progress(
        &format!("{base}/{filename}"),
        &archive,
        cancel,
        128 * 1024 * 1024,
        report,
    )?;
    verify(&archive, digest)?;
    extract(&archive, staging.path(), zip, cancel)?;
    std::fs::remove_file(archive)?;
    let node = find_executable(staging.path(), if zip { "node.exe" } else { "node" })?;
    ensure!(
        compatible(&node, cancel),
        "Downloaded Node failed runtime verification"
    );
    let cli = npm_cli(&node)?;
    let directory = staging.keep();
    if let Err(error) = crate::files::save(
        &parent.join("current.json"),
        &ropey::Rope::from_str(&serde_json::to_string(&node)?),
    ) {
        std::fs::remove_dir_all(directory)?;
        return Err(error);
    }
    Ok((node, cli))
}
pub(super) fn isolate_npm(command: &mut Command, directory: &Path) -> Result<()> {
    let config = directory.join("isolated.npmrc");
    std::fs::write(
        &config,
        "ignore-scripts=true\naudit=false\nfund=false\nregistry=https://registry.npmjs.org\n",
    )?;
    let global = directory.join("global.npmrc");
    std::fs::write(&global, "")?;
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("NPM_CONFIG_")
        {
            command.env_remove(name);
        }
    }
    command
        .env_remove("NODE_OPTIONS")
        .env("NPM_CONFIG_USERCONFIG", &config)
        .env("NPM_CONFIG_GLOBALCONFIG", &global)
        .env("NPM_CONFIG_IGNORE_SCRIPTS", "true");
    Ok(())
}
pub(super) fn validate_lock(directory: &Path) -> Result<()> {
    let lock: Value = serde_json::from_slice(&std::fs::read(directory.join("package-lock.json"))?)?;
    let packages = lock["packages"]
        .as_object()
        .context("npm returned no package lock")?;
    ensure!(packages.len() <= 3000, "Too many package dependencies");
    for (name, package) in packages {
        if name.is_empty() {
            continue;
        }
        let url = url::Url::parse(
            package["resolved"]
                .as_str()
                .context("Dependency has no registry artifact")?,
        )?;
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("registry.npmjs.org")
                && url.username().is_empty(),
            "Dependency URL outside trusted npm registry"
        );
        ensure!(
            package["integrity"]
                .as_str()
                .is_some_and(|s| s.starts_with("sha512-")),
            "Dependency has no SHA-512 integrity"
        );
        ensure!(
            !package["link"].as_bool().unwrap_or(false),
            "Linked npm dependencies are not managed artifacts"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn source_policy_rejects_unapproved_origins_and_paths() {
        for url in [
            "http://registry.npmjs.org/pyright",
            "https://registry.npmjs.org.evil.test/pkg",
            "https://user@registry.npmjs.org/pkg",
            "https://nodejs.org/other/node.zip",
            "https://github.com/attacker/tool/releases/download/x/tool.exe",
            "https://registry.npmjs.org:444/pkg",
        ] {
            assert!(trusted(url).is_err(), "{url}");
        }
        for entry in crate::lsp_catalog::CATALOG {
            assert!(trusted(entry.update_source).is_ok(), "{}", entry.id);
        }
    }
    #[test]
    fn checksums_are_verified_against_actual_bytes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abc").unwrap();
        verify(
            file.path(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        std::fs::write(file.path(), b"abd").unwrap();
        assert!(
            verify(
                file.path(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
            .is_err()
        );
        assert!(verify(file.path(), "unknown").is_err());
    }
    fn archive(name: &str, content: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(file.as_file_mut());
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap();
        file
    }
    #[test]
    fn archive_extraction_rejects_escape_duplicate_and_cancelled_writes() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        for path in ["../escape", "C:/escape", "folder\\escape"] {
            let archive = archive(path, b"untrusted");
            assert!(
                extract(archive.path(), directory.path(), true, &cancel).is_err(),
                "{path}"
            );
        }
        let archive = archive("bin/server", b"verified");
        extract(archive.path(), directory.path(), true, &cancel).unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("bin/server")).unwrap(),
            b"verified"
        );
        assert!(extract(archive.path(), directory.path(), true, &cancel).is_err());
        assert!(
            extract(
                archive.path(),
                directory.path(),
                true,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        let malformed = tempfile::NamedTempFile::new().unwrap();
        assert!(extract(malformed.path(), directory.path(), true, &cancel).is_err());
    }
    #[test]
    fn npm_lock_rejects_untrusted_dependencies_before_installation() {
        let directory = tempfile::tempdir().unwrap();
        let mut lock = json!({"packages":{"":{},"node_modules/server":{"resolved":"https://registry.npmjs.org/server/-/server-1.0.tgz","integrity":"sha512-ZmFrZQ=="}}});
        let write = |lock: &Value| {
            std::fs::write(
                directory.path().join("package-lock.json"),
                serde_json::to_vec(lock).unwrap(),
            )
            .unwrap()
        };
        write(&lock);
        validate_lock(directory.path()).unwrap();
        lock["packages"]["node_modules/server"]["resolved"] =
            json!("https://example.com/server.tgz");
        write(&lock);
        assert!(validate_lock(directory.path()).is_err());
        lock["packages"]["node_modules/server"]["resolved"] =
            json!("https://registry.npmjs.org/server.tgz");
        lock["packages"]["node_modules/server"]["integrity"] = Value::Null;
        write(&lock);
        assert!(validate_lock(directory.path()).is_err());
    }
    #[test]
    fn isolated_npm_configuration_uses_distinct_user_and_global_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new("node");
        isolate_npm(&mut command, directory.path()).unwrap();
        let env: BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|v| (key.to_string_lossy().into_owned(), v.to_owned()))
            })
            .collect();
        assert_ne!(env["NPM_CONFIG_USERCONFIG"], env["NPM_CONFIG_GLOBALCONFIG"]);
        assert!(Path::new(&env["NPM_CONFIG_USERCONFIG"]).is_file());
        assert!(Path::new(&env["NPM_CONFIG_GLOBALCONFIG"]).is_file());
    }
}
