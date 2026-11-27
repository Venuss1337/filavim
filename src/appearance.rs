//! Theme options are independent of GPUI; legacy Moon overrides remain readable.
use serde::Deserialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    Cyberdream,
    RosePineMoon,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    #[default]
    #[serde(alias = "default")]
    Dark,
    Muted,
    Light,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Surfaces {
    pub editor_opacity: f32,
    pub popup_opacity: f32,
    pub sidebar_opacity: f32,
}
impl Default for Surfaces {
    fn default() -> Self {
        Self {
            editor_opacity: 0.,
            popup_opacity: 0.30,
            sidebar_opacity: 0.,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    pub name: Option<ThemeName>,
    pub variant: Variant,
    pub transparent: bool,
    pub saturation: f32,
    pub italic_comments: bool,
    pub surfaces: Surfaces,
}
impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: None,
            variant: Variant::Dark,
            transparent: true,
            saturation: 1.,
            italic_comments: false,
            surfaces: Surfaces::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
    Stretch,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub image: Option<std::path::PathBuf>,
    pub fit: Fit,
    pub opacity: f32,
    pub overlay_opacity: f32,
}
impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            image: None,
            fit: Fit::Cover,
            opacity: 1.,
            overlay_opacity: 0.45,
        }
    }
}
pub fn unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    #[test]
    fn settings_variants_legacy_moon_and_invalid_values() {
        let c=Config::parse("[theme]\nname='cyberdream'\nvariant='muted'\nsaturation=0.8\n[theme.surfaces]\neditor_opacity=0.2\n[background]\nimage='art.png'\nfit='contain'").unwrap();
        assert_eq!(c.appearance.variant, Variant::Muted);
        assert_eq!(c.appearance.name, Some(ThemeName::Cyberdream));
        assert_eq!(c.background.fit, Fit::Contain);
        let old = Config::parse("theme_name='rose-pine-moon'\n[theme]\ntext='#e0def4'").unwrap();
        assert_eq!(old.theme["text"], "#e0def4");
        for text in [
            "[theme]\nname='unknown'",
            "[theme]\nvariant='dawn'",
            "[background]\nopacity=1.1",
            "[theme.surfaces]\npopup_opacity=-0.1",
            "[background]\nfit='tile'",
            "[theme]\nsaturation=nan",
            "[theme]\nvaraint='dark'",
        ] {
            assert!(Config::parse(text).is_err(), "accepted {text}");
        }
    }
    #[test]
    fn settings_persist_with_unrelated_preferences_intact() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "font_size=22\n[cursor]\nsmear=false\n").unwrap();
        crate::setup::save_setting(&path, "theme.name", "cyberdream").unwrap();
        crate::setup::save_setting(&path, "background.image", "C:/art/a b.webp").unwrap();
        let c = crate::setup::save_setting(&path, "theme.surfaces.popup_opacity", "0.96").unwrap();
        assert_eq!(c.font_size, 22.);
        assert!(!c.cursor.smear);
        assert_eq!(c.appearance.surfaces.popup_opacity, 0.96);
        assert_eq!(
            c.background.image.unwrap(),
            std::path::PathBuf::from("C:/art/a b.webp")
        );
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(crate::setup::save_setting(&path, "background.opacity", "2").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
    }
}
