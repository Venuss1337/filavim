//! The 15 official Rosé Pine Moon RGB values. No rendering dependency.
//! <https://rosepinetheme.com/palette/>
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Moon {
    Base,
    Surface,
    Overlay,
    Muted,
    Subtle,
    Text,
    Love,
    Gold,
    Rose,
    Pine,
    Foam,
    Iris,
    HighlightLow,
    HighlightMed,
    HighlightHigh,
}
pub const MOON: [(&str, u32); 15] = [
    ("base", 0x232136),
    ("surface", 0x2a273f),
    ("overlay", 0x393552),
    ("muted", 0x6e6a86),
    ("subtle", 0x908caa),
    ("text", 0xe0def4),
    ("love", 0xeb6f92),
    ("gold", 0xf6c177),
    ("rose", 0xea9a97),
    ("pine", 0x3e8fb0),
    ("foam", 0x9ccfd8),
    ("iris", 0xc4a7e7),
    ("highlight_low", 0x2a283e),
    ("highlight_med", 0x44415a),
    ("highlight_high", 0x56526e),
];
impl Moon {
    pub fn name(self) -> &'static str {
        MOON[self as usize].0
    }
    pub fn rgb(self) -> u32 {
        MOON[self as usize].1
    }
    pub fn resolve(self, overrides: &std::collections::BTreeMap<String, String>) -> u32 {
        overrides
            .get(self.name())
            .and_then(|value| u32::from_str_radix(value.trim_start_matches('#'), 16).ok())
            .filter(|value| MOON.iter().any(|(_, official)| official == value))
            .unwrap_or_else(|| self.rgb())
    }
}

/// Official Cyberdream palettes, upstream colors.lua (MIT, Scott McKendry).
#[derive(Clone, Copy, Debug)]
pub struct Cyber {
    pub bg: u32,
    pub bg_alt: u32,
    pub bg_highlight: u32,
    pub fg: u32,
    pub grey: u32,
    pub blue: u32,
    pub green: u32,
    pub cyan: u32,
    pub red: u32,
    pub yellow: u32,
    pub magenta: u32,
    pub pink: u32,
    pub orange: u32,
    pub purple: u32,
}
impl Cyber {
    pub const DARK: Self = Self {
        bg: 0x16181a,
        bg_alt: 0x1e2124,
        bg_highlight: 0x3c4048,
        fg: 0xffffff,
        grey: 0x7b8496,
        blue: 0x5ea1ff,
        green: 0x5eff6c,
        cyan: 0x5ef1ff,
        red: 0xff6e5e,
        yellow: 0xf1ff5e,
        magenta: 0xff5ef1,
        pink: 0xff5ea0,
        orange: 0xffbd5e,
        purple: 0xbd5eff,
    };
    pub const MUTED: Self = Self {
        blue: 0x78a8e8,
        green: 0x7ee088,
        cyan: 0x7ed8e8,
        red: 0xe8796f,
        yellow: 0xe3c875,
        magenta: 0xe878d8,
        pink: 0xe878a8,
        orange: 0xe3a875,
        purple: 0xb07ee8,
        ..Self::DARK
    };
    pub const LIGHT: Self = Self {
        bg: 0xffffff,
        bg_alt: 0xeaeaea,
        bg_highlight: 0xacacac,
        fg: 0x16181a,
        grey: 0x7b8496,
        blue: 0x0057d1,
        green: 0x008b0c,
        cyan: 0x008c99,
        red: 0xd11500,
        yellow: 0x997b00,
        magenta: 0xd100bf,
        pink: 0xf40064,
        orange: 0xd17c00,
        purple: 0xa018ff,
    };
    pub fn variant(variant: crate::appearance::Variant) -> Self {
        match variant {
            crate::appearance::Variant::Dark => Self::DARK,
            crate::appearance::Variant::Muted => Self::MUTED,
            crate::appearance::Variant::Light => Self::LIGHT,
        }
    }
}
/// Cyberdream util.blend: weight belongs to the first color; channels floor.
pub fn blend(first: u32, second: u32, weight: f32) -> u32 {
    [16, 8, 0].into_iter().fold(0, |rgb, shift| {
        rgb | (((((first >> shift) & 255) as f32 * weight
            + ((second >> shift) & 255) as f32 * (1. - weight))
            .floor() as u32)
            << shift)
    })
}
