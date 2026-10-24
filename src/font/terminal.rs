//! Terminal identity is independent of installation. TERM alone is never proof.
use super::{Environment, Platform, jsonc, read_limited};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    WindowsTerminal,
    WezTerm,
    Kitty,
    Alacritty,
    Other(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Defaults,
    Profile(String),
}
#[derive(Clone, Debug)]
pub struct Target {
    pub path: PathBuf,
    pub scope: Scope,
    pub label: String,
    pub current: Option<String>,
}
#[derive(Debug)]
pub struct Terminal {
    pub kind: Kind,
    pub targets: Vec<Target>,
    pub note: String,
}

pub fn detect(env: &Environment) -> Kind {
    // Remote/multiplexed sessions cannot reliably identify the rendering host.
    if env.get("SSH_CONNECTION").is_some() || env.get("TMUX").is_some() || env.get("STY").is_some()
    {
        return Kind::Other("remote or multiplexed terminal (host unknown)".into());
    }
    if env.get("WT_SESSION").is_some() {
        Kind::WindowsTerminal
    } else if env.get("WEZTERM_PANE").is_some() || env.get("TERM_PROGRAM") == Some("WezTerm") {
        Kind::WezTerm
    } else if env.get("KITTY_WINDOW_ID").is_some() {
        Kind::Kitty
    } else if env.get("ALACRITTY_WINDOW_ID").is_some() || env.get("ALACRITTY_LOG").is_some() {
        Kind::Alacritty
    } else {
        Kind::Other(env.get("TERM_PROGRAM").unwrap_or("unknown").into())
    }
}
pub fn guid(s: &str) -> bool {
    let s = s
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(s);
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn same_guid(a: &str, b: &str) -> bool {
    a.trim_matches(['{', '}'])
        .eq_ignore_ascii_case(b.trim_matches(['{', '}']))
}
pub fn settings_paths(env: &Environment) -> Vec<PathBuf> {
    let Some(local) = env.path("LOCALAPPDATA") else {
        return Vec::new();
    };
    let mut paths = vec![
        local.join("Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json"),
        local.join(
            "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json",
        ),
        local.join("Microsoft/Windows Terminal/settings.json"),
    ];
    // Portable mode (next to the running WindowsTerminal.exe) is only supported
    // through an explicit path choice; an inherited environment cannot prove it.
    paths.retain(|p| p.is_absolute());
    paths
}
fn profiles(root: &jsonc::Node) -> Result<&jsonc::Node> {
    let profiles = root
        .get("profiles")
        .context("No profiles object; open Windows Terminal Settings once, then retry")?;
    ensure!(
        profiles.object,
        "Expected a profiles object; legacy settings require manual configuration"
    );
    if let Some(list) = profiles.get("list") {
        ensure!(list.array, "Expected profiles.list to be an array");
    }
    Ok(profiles)
}
fn profile<'a>(source: &str, root: &'a jsonc::Node, id: &str) -> Option<&'a jsonc::Node> {
    root.get("profiles")?.get("list")?.items.iter().find(|p| {
        p.get("guid")
            .and_then(|n| n.string(source))
            .is_some_and(|s| same_guid(&s, id))
    })
}
fn face(source: &str, node: &jsonc::Node) -> Option<String> {
    node.get("font")
        .and_then(|n| n.get("face"))
        .or_else(|| node.get("fontFace"))
        .and_then(|n| n.string(source))
}
pub fn configured(source: &str, scope: &Scope) -> Result<Option<String>> {
    let root = jsonc::parse(source)?;
    let defaults = profiles(&root)?.get("defaults");
    Ok(match scope {
        Scope::Defaults => defaults.and_then(|n| face(source, n)),
        Scope::Profile(id) => profile(source, &root, id)
            .and_then(|n| face(source, n))
            .or_else(|| defaults.and_then(|n| face(source, n))),
    })
}
pub fn edit(source: &str, scope: &Scope, family: &str) -> Result<String> {
    let root = jsonc::parse(source)?;
    let value = serde_json::to_string(family)?;
    let result = match scope {
        Scope::Defaults => jsonc::set(
            source,
            profiles(&root)?,
            &["defaults", "font", "face"],
            &value,
        )?,
        Scope::Profile(id) => {
            ensure!(guid(id), "Invalid Windows Terminal profile GUID");
            let p = profile(source,&root,id).context("Current profile is not in settings.json; select an explicit scope or use Terminal Settings > Appearance")?;
            jsonc::set(source, p, &["font", "face"], &value)?
        }
    };
    ensure!(
        configured(&result, scope)?.as_deref() == Some(family),
        "Font configuration verification failed"
    );
    Ok(result)
}
pub fn inspect(env: &Environment, platform: Platform) -> Terminal {
    let kind = detect(env);
    let mut terminal = Terminal {
        kind,
        targets: Vec::new(),
        note: String::new(),
    };
    if terminal.kind != Kind::WindowsTerminal || platform != Platform::Windows {
        terminal.note =
            "Automatic configuration unavailable; use the manual instructions below.".into();
        return terminal;
    }
    let current = env.get("WT_PROFILE_ID").filter(|s| guid(s));
    let candidates: Vec<_> = settings_paths(env)
        .into_iter()
        .filter(|p| p.is_file())
        .collect();
    for path in &candidates {
        let result = (|| -> Result<Vec<Target>> {
            let source = read_limited(path, 8 * 1024 * 1024)?;
            let root = jsonc::parse(&source)?;
            profiles(&root)?;
            let mut targets = Vec::new();
            if let Some(id) = current
                && profile(&source, &root, id).is_some()
            {
                let scope = Scope::Profile(id.into());
                targets.push(Target {
                    path: path.clone(),
                    current: configured(&source, &scope)?,
                    scope,
                    label: format!("Current profile {id}"),
                });
            } else {
                let scope = Scope::Defaults;
                targets.push(Target {
                    path: path.clone(),
                    current: configured(&source, &scope)?,
                    scope,
                    label: "Defaults (profiles with overrides keep their font)".into(),
                });
                if let Some(list) = root.get("profiles").and_then(|p| p.get("list")) {
                    for p in &list.items {
                        if let Some(id) = p
                            .get("guid")
                            .and_then(|n| n.string(&source))
                            .filter(|s| guid(s))
                        {
                            let label = p
                                .get("name")
                                .and_then(|n| n.string(&source))
                                .unwrap_or_else(|| id.clone());
                            let scope = Scope::Profile(id);
                            targets.push(Target {
                                path: path.clone(),
                                current: configured(&source, &scope)?,
                                scope,
                                label,
                            });
                        }
                    }
                }
            }
            Ok(targets)
        })();
        match result {
            Ok(t) => terminal.targets.extend(t),
            Err(e) => terminal
                .note
                .push_str(&format!("{}: {e:#}\n", path.display())),
        }
    }
    if candidates.is_empty() {
        terminal.note.push_str("Windows Terminal settings.json was not found. Open Terminal Settings first. Portable/custom settings require manual configuration.");
    } else if candidates.len() > 1 {
        terminal.note.push_str("Several settings files exist. Select the path belonging to the running terminal; no edition was inferred from WT_SESSION.");
    }
    terminal
}
pub fn manual(kind: &Kind, family: &str) -> String {
    let quoted =
        serde_json::to_string(family).unwrap_or_else(|_| "\"OperatorMono Nerd Font\"".into());
    match kind {
        Kind::WindowsTerminal => format!(
            "Windows Terminal: Settings > your profile > Appearance > Font face: {family}. Keep your font size. Reopen the tab, or restart Terminal if the font is not listed."
        ),
        Kind::WezTerm => format!(
            "WezTerm: in the active wezterm.lua, add to the existing config: config.font = wezterm.font({quoted}). Keep font_size. Reload configuration or restart WezTerm. Lua configuration is not edited automatically."
        ),
        Kind::Kitty => format!(
            "Kitty: in the active kitty.conf set font_family {family}. Keep font_size. Reload configuration or restart Kitty. Includes and remote instances are not edited automatically."
        ),
        Kind::Alacritty => format!(
            "Alacritty: in the active alacritty.toml set [font.normal] family = {quoted} and style = \"Medium\". Keep font.size. Reload or restart Alacritty. Imports are not edited automatically."
        ),
        Kind::Other(name) => format!(
            "{name}: open the rendering terminal's font preferences and choose {family}, Medium. Keep your font size; restart the terminal if needed. TERM cannot identify the rendering host."
        ),
    }
}
pub fn validate_target(env: &Environment, path: &Path) -> Result<()> {
    if !settings_paths(env).iter().any(|p| p == path) {
        bail!(
            "Refusing a terminal configuration path outside the supported user settings locations"
        );
    }
    Ok(())
}
