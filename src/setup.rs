//! Confirmed external installations and settings persistence, run on workers.
use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
pub fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}
pub fn executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.into());
    }
    let paths = std::env::var_os("PATH")?;
    let extensions = if cfg!(windows) {
        vec!["", ".exe", ".cmd", ".bat"]
    } else {
        vec![""]
    };
    for dir in std::env::split_paths(&paths) {
        for ext in &extensions {
            let path = dir.join(format!("{name}{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}
struct Captured {
    output: Vec<u8>,
    error: Vec<u8>,
    truncated: bool,
}
fn capture(mut command: Command, timeout: Duration, limit: usize) -> Result<Captured> {
    hide_window(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let reader = |mut stream: Box<dyn Read + Send>, limit: usize| {
        thread::spawn(move || -> std::io::Result<(Vec<u8>, bool)> {
            let mut bytes = Vec::new();
            let mut truncated = false;
            let mut chunk = [0; 8192];
            loop {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                let keep = n.min(limit - bytes.len());
                bytes.extend_from_slice(&chunk[..keep]);
                truncated |= keep != n;
            }
            Ok((bytes, truncated))
        })
    };
    let stdout = reader(Box::new(child.stdout.take().unwrap()), limit);
    let stderr = reader(Box::new(child.stderr.take().unwrap()), 512 * 1024);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Process timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (output, out_cut) = stdout
        .join()
        .map_err(|_| anyhow::anyhow!("Output reader stopped"))??;
    let (error, err_cut) = stderr
        .join()
        .map_err(|_| anyhow::anyhow!("Error reader stopped"))??;
    if !status.success() {
        bail!(
            "Process exited {status}: {}",
            String::from_utf8_lossy(&error)
        );
    }
    Ok(Captured {
        output,
        error,
        truncated: out_cut || err_cut,
    })
}
/// Bounded human-readable output, with an explicit truncation notice.
pub fn run(command: Command, timeout: Duration) -> Result<String> {
    let captured = capture(command, timeout, 512 * 1024)?;
    let mut text = String::from_utf8_lossy(&captured.output).into_owned();
    if !captured.error.is_empty() {
        text.push_str(&String::from_utf8_lossy(&captured.error));
    }
    if captured.truncated {
        text.push_str("\n[output truncated]\n");
    }
    Ok(text)
}
/// Machine data is either complete or an error; stderr never contaminates it.
pub fn run_data(command: Command, timeout: Duration, limit: usize) -> Result<String> {
    let captured = capture(command, timeout, limit)?;
    if captured.truncated {
        bail!("Process output exceeds {limit} bytes; data unavailable");
    }
    String::from_utf8(captured.output).context("Process data is not UTF-8")
}
pub struct Package {
    pub id: &'static str,
    pub executable: &'static str,
    pub description: &'static str,
    pub tool: &'static str,
    pub args: &'static [&'static str],
}
pub const PACKAGES: &[Package] = &[
    Package {
        id: "rust",
        executable: "rust-analyzer",
        description: "Rust analyzer (rustup component)",
        tool: "rustup",
        args: &["component", "add", "rust-analyzer", "rust-src"],
    },
    Package {
        id: "python",
        executable: "pyright-langserver",
        description: "Pyright (requires Node.js/npm)",
        tool: "npm",
        args: &["install", "--global", "pyright"],
    },
    Package {
        id: "typescript",
        executable: "typescript-language-server",
        description: "TypeScript / JavaScript (requires Node.js/npm)",
        tool: "npm",
        args: &[
            "install",
            "--global",
            "typescript-language-server",
            "typescript",
        ],
    },
    Package {
        id: "clangd",
        executable: "clangd",
        description: "C/C++: install LLVM/clangd from llvm.org or your OS package manager",
        tool: "",
        args: &[],
    },
    Package {
        id: "go",
        executable: "gopls",
        description: "Go language server (requires Go)",
        tool: "go",
        args: &["install", "golang.org/x/tools/gopls@latest"],
    },
    Package {
        id: "json",
        executable: "vscode-json-language-server",
        description: "JSON (requires Node.js/npm)",
        tool: "npm",
        args: &["install", "--global", "vscode-langservers-extracted"],
    },
];
pub fn installed() -> Vec<(String, String)> {
    PACKAGES
        .iter()
        .map(|p| {
            (
                p.id.into(),
                executable(p.executable).map_or_else(
                    || format!("Missing - {}", p.description),
                    |v| format!("Installed: {}", v.display()),
                ),
            )
        })
        .collect()
}
pub fn install(id: &str) -> Result<String> {
    if id == "font-fira-code" {
        return install_font();
    }
    let p = PACKAGES
        .iter()
        .find(|p| p.id == id)
        .context("Unknown supported package")?;
    if p.id == "clangd" {
        bail!(
            "{}; then configure lsp.servers in config.toml. No administrator package manager is run automatically.",
            p.description
        )
    }
    let tool = executable(p.tool)
        .with_context(|| format!("Install {} first, then retry. {}", p.tool, p.description))?;
    let mut command = Command::new(tool);
    command.args(p.args);
    let output = run(command, Duration::from_secs(600))?;
    Ok(format!(
        "{} installation/update finished. {}\nIf not detected, add its install directory to PATH or set the server command path in config.toml.\n{output}",
        p.id, p.description
    ))
}
fn install_font() -> Result<String> {
    // Only the OFL-licensed Fira Code archive is downloaded. Never fetch MonoLisa.
    if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile","-NonInteractive","-Command",r#"$ErrorActionPreference='Stop'; $stage=Join-Path $env:TEMP ('filavim-font-'+[guid]::NewGuid()); New-Item -ItemType Directory -Path $stage | Out-Null; Invoke-WebRequest -Uri 'https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip' -OutFile (Join-Path $stage 'font.zip'); Expand-Archive -LiteralPath (Join-Path $stage 'font.zip') -DestinationPath (Join-Path $stage 'unpacked'); $dest=Join-Path $env:LOCALAPPDATA 'Microsoft\Windows\Fonts'; New-Item -ItemType Directory -Force -Path $dest | Out-Null; New-Item -Path 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Fonts' -Force | Out-Null; Get-ChildItem -LiteralPath (Join-Path $stage 'unpacked\ttf') -Filter '*.ttf' | ForEach-Object { $target=Join-Path $dest $_.Name; Copy-Item -LiteralPath $_.FullName -Destination $target; New-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Fonts' -Name ($_.BaseName+' (TrueType)') -Value $target -PropertyType String -Force | Out-Null }; Get-ChildItem -LiteralPath (Join-Path $stage 'unpacked') -Recurse -Filter '*OFL*' | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $dest 'FiraCode-OFL.txt') }; Write-Output 'Fira Code installed per-user. Restart your terminal and choose Fira Code in its font settings.'"#]);
        run(command, Duration::from_secs(180))
    } else {
        if executable("curl").is_none() || executable("unzip").is_none() {
            bail!(
                "Install curl and unzip, or download Fira Code from https://github.com/tonsky/FiraCode/releases and install the TTF files manually."
            )
        }
        let mut command = Command::new("sh");
        command.args(["-c",r#"set -eu
stage=$(mktemp -d)
curl -fL https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip -o "$stage/font.zip"
unzip -q "$stage/font.zip" -d "$stage/unpacked"
if [ "$(uname)" = Darwin ]; then dest="$HOME/Library/Fonts"; else dest="${XDG_DATA_HOME:-$HOME/.local/share}/fonts/FiraCode"; fi
mkdir -p "$dest"
cp "$stage"/unpacked/ttf/*.ttf "$dest/"
find "$stage/unpacked" -iname '*OFL*' -exec cp '{}' "$dest/FiraCode-OFL.txt" \;
if command -v fc-cache >/dev/null 2>&1; then fc-cache -f "$dest"; fi
printf 'Fira Code installed. Choose it in your terminal font settings.\n'
"#]);
        run(command, Duration::from_secs(180))
    }
}
pub fn save_setting(path: &Path, key: &str, value: &str) -> Result<crate::config::Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut document: toml::Value = toml::from_str(&text)?;
    if let Some(rest) = key.strip_prefix("server.") {
        let (name, property) = rest
            .split_once('.')
            .context("Use server.NAME.command/enabled/languages")?;
        let mut config = crate::config::Config::parse(&text)?;
        let server = config
            .lsp
            .servers
            .iter_mut()
            .find(|s| s.name == name)
            .context("Unknown configured language server")?;
        match property {
            "command" => server.command = value.trim_matches('"').to_string(),
            "enabled" => server.enabled = value.parse()?,
            "languages" => {
                server.languages = value
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            _ => bail!("Use command, enabled, or languages"),
        }
        let table = document
            .as_table_mut()
            .unwrap()
            .entry("lsp")
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .context("lsp must be a table")?;
        table.insert("servers".into(), toml::Value::try_from(config.lsp.servers)?);
        let output = toml::to_string_pretty(&document)?;
        let config = crate::config::Config::parse(&output)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?
        }
        crate::files::save(path, &ropey::Rope::from_str(&output))?;
        return Ok(config);
    }
    let parsed: toml::Value = toml::from_str::<toml::Value>(&format!("value = {value}"))
        .ok()
        .and_then(|mut v| v.as_table_mut()?.remove("value"))
        .unwrap_or_else(|| toml::Value::String(value.into()));
    let parts = key.split('.').collect::<Vec<_>>();
    let mut table = document
        .as_table_mut()
        .context("Configuration must be a TOML table")?;
    for part in &parts[..parts.len().saturating_sub(1)] {
        table = table
            .entry((*part).to_owned())
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .context("Setting parent is not a table")?;
    }
    table.insert(parts.last().context("Empty setting")?.to_string(), parsed);
    let output = toml::to_string_pretty(&document)?;
    let config = crate::config::Config::parse(&output)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?
    }
    crate::files::save(path, &ropey::Rope::from_str(&output))?;
    Ok(config)
}
