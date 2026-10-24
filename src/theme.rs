use crate::config::Config;
use anyhow::{Result, bail};
use ratatui::style::Color;

#[derive(Clone)]
pub struct Theme {
    pub base: Color,
    pub surface: Color,
    pub overlay: Color,
    pub text: Color,
    pub muted: Color,
    pub iris: Color,
    pub love: Color,
    pub gold: Color,
    pub foam: Color,
    pub pine: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            base: Color::Rgb(35, 33, 54),
            surface: Color::Rgb(42, 39, 63),
            overlay: Color::Rgb(57, 53, 82),
            text: Color::Rgb(224, 222, 244),
            muted: Color::Rgb(110, 106, 134),
            iris: Color::Rgb(196, 167, 231),
            love: Color::Rgb(235, 111, 146),
            gold: Color::Rgb(246, 193, 119),
            foam: Color::Rgb(156, 207, 216),
            pine: Color::Rgb(62, 143, 176),
        }
    }
}
impl Theme {
    pub fn for_terminal(mut self, mode: crate::config::ColorMode) -> Self {
        use crate::config::ColorMode;
        let mode = if mode == ColorMode::Auto {
            let term = std::env::var("TERM").unwrap_or_default();
            let color = std::env::var("COLORTERM").unwrap_or_default();
            if cfg!(windows)
                || color.contains("truecolor")
                || color.contains("24bit")
                || term.contains("direct")
            {
                ColorMode::Truecolor
            } else if term.contains("256color") {
                ColorMode::Indexed
            } else {
                ColorMode::Ansi
            }
        } else {
            mode
        };
        for color in [
            &mut self.base,
            &mut self.surface,
            &mut self.overlay,
            &mut self.text,
            &mut self.muted,
            &mut self.iris,
            &mut self.love,
            &mut self.gold,
            &mut self.foam,
            &mut self.pine,
        ] {
            if let Color::Rgb(r, g, b) = *color {
                *color = match mode {
                    ColorMode::Indexed => {
                        let cube = |v: u8| ((u16::from(v) * 5 + 127) / 255) as u8;
                        let gray = ((u16::from(r) + u16::from(g) + u16::from(b)) / 3) as u8;
                        if r.abs_diff(g) < 12 && g.abs_diff(b) < 12 {
                            Color::Indexed(232 + ((u16::from(gray) * 23) / 255) as u8)
                        } else {
                            Color::Indexed(16 + 36 * cube(r) + 6 * cube(g) + cube(b))
                        }
                    }
                    ColorMode::Ansi => {
                        let palette = [
                            (Color::Black, (0, 0, 0)),
                            (Color::DarkGray, (85, 85, 85)),
                            (Color::White, (170, 170, 170)),
                            (Color::Gray, (225, 225, 225)),
                            (Color::Blue, (60, 80, 160)),
                            (Color::Magenta, (170, 80, 170)),
                            (Color::Cyan, (80, 170, 170)),
                            (Color::Yellow, (210, 170, 90)),
                            (Color::Red, (210, 90, 120)),
                            (Color::Green, (80, 160, 100)),
                        ];
                        palette
                            .into_iter()
                            .min_by_key(|(_, (pr, pg, pb))| {
                                let dr = i32::from(r) - pr;
                                let dg = i32::from(g) - pg;
                                let db = i32::from(b) - pb;
                                dr * dr + dg * dg + db * db
                            })
                            .unwrap()
                            .0
                    }
                    _ => *color,
                };
            }
        }
        self
    }
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut theme = Self::default();
        if config.theme_name == "high-contrast" {
            theme.base = Color::Black;
            theme.surface = Color::Rgb(20, 20, 28);
            theme.text = Color::White;
            theme.muted = Color::Rgb(160, 160, 180);
        }
        for (name, hex) in &config.theme {
            if hex.len() != 7
                || !hex.starts_with('#')
                || !hex[1..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                bail!("theme.{name} must be a #rrggbb color");
            }
            let value = u32::from_str_radix(&hex[1..], 16)?;
            let color = Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8);
            match name.as_str() {
                "base" => theme.base = color,
                "surface" => theme.surface = color,
                "overlay" => theme.overlay = color,
                "text" => theme.text = color,
                "muted" => theme.muted = color,
                "iris" => theme.iris = color,
                "love" => theme.love = color,
                "gold" => theme.gold = color,
                "foam" => theme.foam = color,
                "pine" => theme.pine = color,
                _ => bail!("Unknown theme token: {name}"),
            }
        }
        Ok(theme)
    }
}
