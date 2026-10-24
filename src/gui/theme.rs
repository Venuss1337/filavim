use crate::config::Config;
use gpui::{Hsla, rgb};
pub struct Theme {
    pub base: Hsla,
    pub surface: Hsla,
    pub muted: Hsla,
    pub text: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub error: Hsla,
}
impl Theme {
    pub fn new(c: &Config) -> Self {
        let color = |key: &str, default: u32| {
            rgb(c
                .theme
                .get(key)
                .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
                .unwrap_or(default))
            .into()
        };
        Self {
            base: color("base", 0x232136),
            surface: color("surface", 0x2a273f),
            muted: color("muted", 0x6e6a86),
            text: color("text", 0xe0def4),
            accent: color("iris", 0xc4a7e7),
            selection: color("overlay", 0x393552),
            error: color("love", 0xeb6f92),
        }
    }
}
