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

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Easing {
    Linear,
    Quadratic,
    #[default]
    Cubic,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    #[default]
    Auto,
    Truecolor,
    Indexed,
    Ansi,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisualConfig {
    pub cursor_effect: CursorEffect,
    pub pixiedust: PixiedustConfig,
    pub smear: SmearConfig,
    pub cursor_animation: Option<bool>,
    pub smooth_scroll: Option<bool>,
    pub cursor_duration_ms: Option<u64>,
    pub trail_intensity: Option<u8>,
    pub scroll_duration_ms: u64,
    pub easing: Easing,
    pub fps: Option<u16>,
    pub colors: ColorMode,
}
impl Default for VisualConfig {
    fn default() -> Self {
        Self {
            cursor_effect: CursorEffect::Auto,
            pixiedust: PixiedustConfig::default(),
            cursor_animation: None,
            smear: SmearConfig::default(),
            smooth_scroll: None,
            cursor_duration_ms: None,
            trail_intensity: None,
            scroll_duration_ms: 120,
            easing: Easing::Cubic,
            fps: None,
            colors: ColorMode::Auto,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Effects {
    pub cursor_effect: CursorEffect,
    pub pixiedust: PixiedustConfig,
    pub smear: SmearConfig,
    pub cursor: bool,
    pub scroll: bool,
    pub scroll_ms: u64,
    pub trail: u8,
    pub fps: u16,
    pub easing: Easing,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorEffect {
    #[default]
    Auto,
    Pixiedust,
    Smear,
    Off,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PixiedustConfig {
    pub density: f32,
    pub lifetime_ms: u16,
    pub fps: u16,
}
impl Default for PixiedustConfig {
    fn default() -> Self {
        Self {
            density: 0.7,
            lifetime_ms: 500,
            fps: 60,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorQuality {
    #[default]
    Blocks,
    Legacy,
    Ascii,
    Native,
}
/// Defaults ported from smear-cursor.nvim (17 ms reference timestep).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SmearConfig {
    pub frame_interval_ms: Option<u64>,
    pub stiffness: f64,
    pub trailing_stiffness: f64,
    pub damping: f64,
    pub anticipation: f64,
    pub trailing_exponent: f64,
    pub max_length: f64,
    pub gradient_exponent: f64,
    #[serde(deserialize_with = "cursor_color")]
    pub color: Option<ratatui::style::Color>,
    pub quality: CursorQuality,
}
impl Default for SmearConfig {
    fn default() -> Self {
        Self {
            frame_interval_ms: None,
            stiffness: 0.6,
            trailing_stiffness: 0.45,
            damping: 0.85,
            anticipation: 0.2,
            trailing_exponent: 3.0,
            max_length: 25.0,
            gradient_exponent: 1.0,
            color: None,
            quality: CursorQuality::Blocks,
        }
    }
}
fn cursor_color<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<ratatui::style::Color>, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() != 7 || !s.starts_with('#') || !s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom("cursor color must be #rrggbb"));
    }
    let rgb = u32::from_str_radix(&s[1..], 16).map_err(serde::de::Error::custom)?;
    Ok(Some(ratatui::style::Color::Rgb(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    )))
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
pub struct Config {
    pub lsp: crate::lsp::Config,
    pub git: crate::git::Config,
    pub terminal: crate::terminal::Config,
    pub snippets: BTreeMap<String, BTreeMap<String, String>>,
    pub theme_name: String,
    pub status_bar: bool,
    pub borders: bool,
    pub nerd_fonts: bool,
    pub tab_width: usize,
    pub expand_tabs: bool,
    pub auto_indent: bool,
    pub line_numbers: bool,
    pub unicode: bool,
    pub clipboard_osc52: bool,
    pub preset: Preset,
    pub syntax: bool,
    pub syntax_max_bytes: usize,
    pub theme: BTreeMap<String, String>,
    /// Mode -> key chord/sequence -> action name. E.g. normal."space w" = "save".
    pub keybindings: BTreeMap<String, BTreeMap<String, String>>,
    pub visual: VisualConfig,
    pub navigation: NavigationConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lsp: crate::lsp::Config::default(),
            git: crate::git::Config::default(),
            terminal: crate::terminal::Config::default(),
            snippets: BTreeMap::new(),
            theme_name: "rose-pine-moon".into(),
            status_bar: true,
            borders: true,
            nerd_fonts: false,
            tab_width: 4,
            expand_tabs: true,
            auto_indent: true,
            line_numbers: true,
            unicode: true,
            clipboard_osc52: true,
            preset: Preset::Balanced,
            syntax: true,
            syntax_max_bytes: 2 * 1024 * 1024,
            theme: BTreeMap::new(),
            keybindings: BTreeMap::new(),
            visual: VisualConfig::default(),
            navigation: NavigationConfig::default(),
        }
    }
}
impl Config {
    pub fn effects(&self) -> Effects {
        let (cursor, scroll, trail, fps) = match self.preset {
            Preset::Beautiful => (true, true, 10, 60),
            Preset::Balanced => (true, false, 7, 60),
            Preset::Minimal => (false, false, 7, 30),
        };
        let mut smear = self.visual.smear;
        if !self.unicode && smear.quality != CursorQuality::Native {
            smear.quality = CursorQuality::Ascii;
        }
        Effects {
            cursor_effect: self.visual.cursor_effect,
            pixiedust: self.visual.pixiedust,
            smear,
            cursor: self.visual.cursor_animation.unwrap_or(cursor)
                && self.visual.smear.quality != CursorQuality::Native
                && self.visual.cursor_effect != CursorEffect::Off,
            scroll: self.visual.smooth_scroll.unwrap_or(scroll),
            scroll_ms: self.visual.scroll_duration_ms,
            trail: self.visual.trail_intensity.unwrap_or(trail),
            fps: self.visual.fps.unwrap_or(fps),
            easing: self.visual.easing,
        }
    }
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("FILAVIM_CONFIG") {
            return path.into();
        }
        if cfg!(windows)
            && let Some(dir) = std::env::var_os("APPDATA")
        {
            return PathBuf::from(dir).join("filavim/config.toml");
        }
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(dir).join("filavim/config.toml");
        }
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(".config/filavim/config.toml")
    }
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("Invalid configuration: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        if !(1..=16).contains(&config.tab_width) {
            bail!("tab_width must be between 1 and 16");
        }
        let effects = config.effects();
        let server_names = config
            .lsp
            .servers
            .iter()
            .map(|s| &s.name)
            .collect::<std::collections::BTreeSet<_>>();
        if config.lsp.debounce_ms > 2000
            || !(100..=120_000).contains(&config.lsp.timeout_ms)
            || config.lsp.servers.iter().any(|s| {
                s.command.trim().is_empty()
                    || s.name.trim().is_empty()
                    || s.languages.is_empty()
                    || s.languages.iter().any(|l| l.trim().is_empty())
            })
            || config.terminal.scrollback > 100_000
            || !(4..=80).contains(&config.terminal.height)
            || config.terminal.shell.trim().is_empty()
            || config.git.debounce_ms > 5000
            || !(1000..=3_600_000).contains(&config.git.refresh_ms)
            || server_names.len() != config.lsp.servers.len()
            || !["rose-pine-moon", "high-contrast"].contains(&config.theme_name.as_str())
        {
            bail!(
                "Invalid IDE configuration: check LSP commands/timeouts, terminal bounds, Git timing, and theme_name"
            );
        }
        let s = effects.smear;
        let p = effects.pixiedust;
        if !p.density.is_finite()
            || !(0.0..=4.0).contains(&p.density)
            || !(100..=1500).contains(&p.lifetime_ms)
            || !(10..=120).contains(&p.fps)
        {
            bail!("Pixiedust requires density 0..4, lifetime_ms 100..1500, fps 10..120");
        }
        if ![s.stiffness, s.trailing_stiffness, s.damping]
            .into_iter()
            .all(|v| v.is_finite() && v > 0.0 && v < 1.0)
            || !s.anticipation.is_finite()
            || !(0.0..=1.0).contains(&s.anticipation)
            || !s.trailing_exponent.is_finite()
            || !(0.1..=10.0).contains(&s.trailing_exponent)
            || !s.gradient_exponent.is_finite()
            || !(0.1..=10.0).contains(&s.gradient_exponent)
            || !s.max_length.is_finite()
            || !(2.0..=100.0).contains(&s.max_length)
            || s.frame_interval_ms
                .is_some_and(|ms| !(4..=1000).contains(&ms))
        {
            bail!(
                "Invalid smear physics: stiffness/damping must be in (0,1), frame interval 4..1000 ms"
            );
        }
        if !(1..=240).contains(&effects.fps)
            || config
                .visual
                .cursor_duration_ms
                .is_some_and(|ms| !(16..=500).contains(&ms))
            || !(16..=500).contains(&effects.scroll_ms)
            || effects.trail > 10
        {
            bail!("Visual options require fps 1..240, durations 16..500 ms, trail_intensity 0..10");
        }
        let n = &config.navigation;
        if !(1..=2_000_000).contains(&n.max_files)
            || !(1..=2000).contains(&n.max_results)
            || n.debounce_ms > 1000
            || n.grep_max_bytes == 0
            || n.preview_max_bytes == 0
            || !(16..=80).contains(&n.explorer_width)
        {
            bail!("Invalid navigation limit, debounce, or explorer width");
        }
        crate::theme::Theme::from_config(&config)?;
        crate::input::Keymap::new(&config)?;
        Ok(config)
    }
}
