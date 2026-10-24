//! Native/core settings, isolated from the terminal frontend configuration.
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
pub struct Config {
    pub tab_width: usize,
    pub expand_tabs: bool,
    pub auto_indent: bool,
    pub line_numbers: bool,
    pub theme_name: String,
    pub theme: BTreeMap<String, String>,
    pub keybindings: BTreeMap<String, BTreeMap<String, String>>,
    pub preset: Preset,
    pub font_size: f32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: 4,
            expand_tabs: true,
            auto_indent: true,
            line_numbers: true,
            theme_name: "rose-pine-moon".into(),
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
        let c: Self = toml::from_str(s)?;
        if !(1..=16).contains(&c.tab_width)
            || !c.font_size.is_finite()
            || !(8.0..=48.0).contains(&c.font_size)
        {
            bail!("tab_width must be 1..16 and font_size 8..48");
        }
        if c.theme_name != "rose-pine-moon" {
            bail!("Checkpoint 1 supports rose-pine-moon");
        }
        for (key, value) in &c.theme {
            if ![
                "base", "surface", "overlay", "muted", "subtle", "text", "love", "gold", "rose",
                "pine", "foam", "iris",
            ]
            .contains(&key.as_str())
                || value.len() != 7
                || !value.starts_with('#')
                || u32::from_str_radix(&value[1..], 16).is_err()
            {
                bail!("Invalid theme color {key}");
            }
        }
        crate::input::Keymap::new(&c)?;
        Ok(c)
    }
}
