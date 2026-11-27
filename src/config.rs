//! Native/core settings, isolated from the terminal frontend configuration.
//!
//! [`Config::load`] reads the path selected by [`Config::path`], honoring
//! `FILAVIM_GPUI_CONFIG` for isolated tests. Parsing validates unknown fields and
//! compatibility migrations; appearance and LSP installer preferences have their
//! own modules. Loading defaults does not install tools or start services.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Beautiful,
    #[default]
    Balanced,
    Minimal,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NavigationConfig {
    pub show_hidden: bool,
    pub respect_ignore: bool,
    pub max_files: usize,
    pub max_results: usize,
    pub grep_max_bytes: u64,
    pub preview_max_bytes: u64,
    pub debounce_ms: u64,
    pub explorer_width: u16,
}
impl Default for NavigationConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            respect_ignore: true,
            max_files: 200_000,
            max_results: 300,
            grep_max_bytes: 8 * 1024 * 1024,
            preview_max_bytes: 512 * 1024,
            debounce_ms: 60,
            explorer_width: 28,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompletionConfig {
    pub enabled: bool,
    pub minimum_prefix: usize,
    pub matching: String,
    pub auto_trigger: bool,
    pub accept_enter: bool,
    pub accept_tab: bool,
}
impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_prefix: 1,
            matching: "fuzzy".into(),
            auto_trigger: true,
            accept_enter: true,
            accept_tab: true,
        }
    }
}

/// Cursor presentation only; never changes logical editing or modal behavior.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    pub enabled: bool,
    pub smooth: bool,
    pub smear: bool,
    pub reduced_motion: bool,
    pub duration_ms: u64,
    pub trail_size: f32,
    pub short_duration_ms: u64,
    pub animate_in_insert_mode: bool,
    pub fps: u32,
}
impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smooth: true,
            smear: true,
            reduced_motion: false,
            duration_ms: 120,
            short_duration_ms: 40,
            trail_size: 0.85,
            animate_in_insert_mode: true,
            fps: 120,
        }
    }
}
impl CursorConfig {
    pub fn active(&self) -> bool {
        self.enabled && !self.reduced_motion && self.smooth
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub lsp: crate::lsp::Config,
    pub completion: CompletionConfig,
    pub pairs: crate::pairs::Config,
    pub cursor: CursorConfig,
    pub git: crate::git::Config,
    pub navigation: NavigationConfig,
    pub syntax: bool,
    pub syntax_max_bytes: usize,
    pub tab_width: usize,
    pub expand_tabs: bool,
    pub auto_indent: bool,
    pub detect_indentation: bool,
    pub line_numbers: bool,
    pub theme_name: String,
    #[serde(skip)]
    pub appearance: crate::appearance::ThemeConfig,
    pub background: crate::appearance::BackgroundConfig,
    pub theme: BTreeMap<String, String>,
    pub keybindings: BTreeMap<String, BTreeMap<String, String>>,
    pub preset: Preset,
    pub font_size: f32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            lsp: crate::lsp::Config::default(),
            completion: CompletionConfig::default(),
            pairs: crate::pairs::Config::default(),
            cursor: CursorConfig::default(),
            git: crate::git::Config::default(),
            navigation: NavigationConfig::default(),
            syntax: true,
            syntax_max_bytes: 2 * 1024 * 1024,
            tab_width: 4,
            expand_tabs: true,
            auto_indent: true,
            detect_indentation: true,
            line_numbers: true,
            theme_name: "cyberdream".into(),
            appearance: Default::default(),
            background: Default::default(),
            theme: BTreeMap::new(),
            keybindings: BTreeMap::new(),
            preset: Preset::Balanced,
            font_size: 18.0,
        }
    }
}
impl Config {
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("FILAVIM_GPUI_CONFIG") {
            return p.into();
        }
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
        };
        base.map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("filavim-gpui/config.toml")
    }
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s)
                .with_context(|| format!("Invalid configuration: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        let mut document: toml::Value = toml::from_str(s)?;
        // Read compatibility only: obsolete particle options never enter runtime settings.
        if let Some(cursor) = document
            .get_mut("cursor")
            .and_then(toml::Value::as_table_mut)
        {
            cursor.remove("pixiedust");
            cursor.remove("intensity");
        }
        let mut options = toml::map::Map::new();
        if let Some(theme) = document
            .get_mut("theme")
            .and_then(toml::Value::as_table_mut)
        {
            for key in [
                "name",
                "variant",
                "transparent",
                "saturation",
                "italic_comments",
                "surfaces",
            ] {
                if let Some(value) = theme.remove(key) {
                    options.insert(key.into(), value);
                }
            }
        }
        let appearance: crate::appearance::ThemeConfig = toml::Value::Table(options).try_into()?;
        let mut c: Self = document.try_into()?;
        c.appearance = appearance;
        if c.completion.minimum_prefix > 256
            || !matches!(
                c.completion.matching.as_str(),
                "prefix" | "substring" | "fuzzy"
            )
        {
            bail!(
                "completion.minimum_prefix must be 0..256 and matching prefix, substring or fuzzy"
            );
        }
        if c.pairs.languages.values().any(|pairs| {
            pairs.len() > 32
                || pairs
                    .iter()
                    .any(|p| p.chars().count() != 2 || p.chars().any(char::is_control))
        }) {
            bail!("pairs.languages entries must contain at most 32 two-character pairs");
        }
        if !(1..=16).contains(&c.tab_width)
            || !c.font_size.is_finite()
            || !(8.0..=48.0).contains(&c.font_size)
        {
            bail!("tab_width must be 1..16 and font_size 8..48");
        }
        if !(16..=250).contains(&c.cursor.duration_ms)
            || !(16..=250).contains(&c.cursor.short_duration_ms)
            || !c.cursor.trail_size.is_finite()
            || !(0.0..=1.0).contains(&c.cursor.trail_size)
            || !(24..=120).contains(&c.cursor.fps)
        {
            bail!(
                "cursor duration_ms/short_duration_ms must be 16..250, trail_size 0..1 and fps 24..120"
            );
        }
        if !matches!(c.theme_name.as_str(), "rose-pine-moon" | "cyberdream") {
            bail!("Theme must be cyberdream or rose-pine-moon");
        }
        let surface = &c.appearance.surfaces;
        if ![
            c.appearance.saturation,
            surface.editor_opacity,
            surface.popup_opacity,
            surface.sidebar_opacity,
            c.background.opacity,
            c.background.overlay_opacity,
        ]
        .into_iter()
        .all(crate::appearance::unit)
        {
            bail!("Saturation and theme/background opacity values must be finite and within 0..1");
        }
        for (key, value) in &c.theme {
            if !crate::palette::MOON
                .iter()
                .any(|(name, _)| *name == key.as_str())
                || value.len() != 7
                || !value.starts_with('#')
                || u32::from_str_radix(&value[1..], 16).is_err()
            {
                bail!("Invalid theme color {key}");
            }
        }
        if c.lsp.timeout_ms < 100
            || c.lsp.timeout_ms > 120000
            || c.lsp.debounce_ms > 2000
            || c.navigation.max_files == 0
            || c.navigation.max_files > 2_000_000
            || c.navigation.max_results == 0
            || c.navigation.max_results > 2000
            || c.navigation.grep_max_bytes == 0
            || c.navigation.preview_max_bytes == 0
            || c.lsp.servers.iter().any(|s| {
                s.name.trim().is_empty() || s.command.trim().is_empty() || s.languages.is_empty()
            })
        {
            bail!("Invalid language server/navigation limits");
        }
        let names = c
            .lsp
            .servers
            .iter()
            .map(|s| &s.name)
            .collect::<std::collections::BTreeSet<_>>();
        if names.len() != c.lsp.servers.len() {
            bail!("Duplicate server names");
        }
        crate::input::Keymap::new(&c)?;
        Ok(c)
    }
}
