//! GPUI semantic colors resolved from shared palettes and appearance settings.
//!
//! [`crate::palette`] owns palette RGB values; [`crate::appearance`] owns portable
//! settings. Resolve [`Theme`] when settings change, never while painting rows.
//!
//! Lucent separates three families of roles (see DESIGN.md §4):
//! - **Ink** (`text`, `secondary`, `muted`, `faint`): one hue at four emphasis
//!   levels, always drawn at full alpha over any material.
//! - **Materials** (`glass_*`, `float_*`, `lens_*`, `well`, `rim`, `edge`, `sheen`):
//!   the light-bearing surfaces. Materials are translucent by design.
//! - **Signals** (`accent`, mode colors, severities): color that means something.
use crate::{
    appearance::Glass,
    config::Config,
    palette::{Cyber, Lucent, Moon, blend},
    syntax::Token,
};
use gpui::{Hsla, rgb};
/// Categorical palette hues for identity colors (file types, completion kinds).
/// Never use a hue to communicate state; state uses the semantic roles below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Blue,
    Green,
    Cyan,
    Yellow,
    Magenta,
    Orange,
    Purple,
}
/// Resolved once on settings changes; the renderer uses semantic roles only.
#[derive(Clone, Copy)]
pub struct Theme {
    pub light: bool,
    /// Effective slab material after `theme.transparent` is applied.
    pub glass: Glass,
    // Ink, brightest to faintest.
    pub text: Hsla,
    pub secondary: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    /// The interface signal: keyboard focus, primary actions, unsaved state.
    pub accent: Hsla,
    // World: what lies behind the slab.
    pub base: Hsla,
    /// Tint laid over the wallpaper everywhere (`background.overlay_opacity`).
    pub world_tint: Hsla,
    // Materials.
    /// Main slab tint, top and bottom of its vertical gradient.
    pub glass_top: Hsla,
    pub glass_bottom: Hsla,
    /// Floating glass (popups, pickers, toasts, dialogs): near-opaque.
    pub float_top: Hsla,
    pub float_bottom: Hsla,
    /// Lit upper rim and the dim outline it fades into.
    pub rim: Hsla,
    pub edge: Hsla,
    /// Faint highlight just inside a lit top edge.
    pub sheen: Hsla,
    /// Recessed groove (tab well, segmented tracks, inputs).
    pub well: Hsla,
    pub well_edge: Hsla,
    /// The lens: a raised, brighter pane that marks the current selection.
    pub lens_top: Hsla,
    pub lens_bottom: Hsla,
    pub lens_edge: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub hairline: Hsla,
    pub scrim: Hsla,
    pub shadow: Hsla,
    // Compatibility roles still read by older surfaces.
    pub overlay: Hsla,
    pub popup_border: Hsla,
    pub popup_selection: Hsla,
    // Signals.
    pub error: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub hint: Hsla,
    // Editor.
    pub selection: Hsla,
    pub active_line: Hsla,
    pub gutter: Hsla,
    pub cursor: Hsla,
    pub matched: Hsla,
    pub punctuation: Hsla,
    pub search: Hsla,
    pub italic_comments: bool,
    hues: [Hsla; 7],
    string: Hsla,
    number: Hsla,
    function: Hsla,
    keyword: Hsla,
    type_color: Hsla,
    constant: Hsla,
    boolean: Hsla,
    operator: Hsla,
    statement: Hsla,
    preprocessor: Hsla,
    kinds: [Hsla; 26],
    modes: [Hsla; 4],
}
/// Palette-specific inputs; everything else is derived identically.
struct Source {
    light: bool,
    night: u32,
    glass_top: u32,
    glass_bottom: u32,
    ink: [u32; 4],
    accent: u32,
    error: u32,
    success: u32,
    warning: u32,
    info: u32,
    hint: u32,
    search: u32,
    selection: u32,
    punctuation: u32,
    hues: [u32; 7],
    /// String, number, function, keyword, type, constant, boolean, operator,
    /// statement, preprocessor.
    syntax: [u32; 10],
    modes: [u32; 4],
    kinds: [u32; 26],
}
fn lucent(p: Lucent, light: bool) -> Source {
    let mut kinds = [p.ink2; 26];
    for (kind, value) in [
        (1, p.ink3),
        (2, p.ice),
        (3, p.ice),
        (4, p.ice),
        (5, p.aqua),
        (6, p.ink2),
        (7, p.aqua),
        (8, p.aqua),
        (9, p.lilac),
        (10, p.aqua),
        (11, p.gold),
        (12, p.peach),
        (13, p.aqua),
        (14, p.lilac),
        (15, p.mint),
        (16, p.gold),
        (17, p.ink2),
        (18, p.ink2),
        (19, p.ink2),
        (20, p.peach),
        (21, p.peach),
        (22, p.aqua),
        (23, p.rose),
        (24, p.slate),
        (25, p.aqua),
    ] {
        kinds[kind] = value;
    }
    Source {
        light,
        night: p.night,
        glass_top: p.glass_top,
        glass_bottom: p.glass_bottom,
        ink: [p.ink, p.ink2, p.ink3, p.ink4],
        accent: p.ice,
        error: p.coral,
        success: p.mint,
        warning: p.amber,
        info: p.ice,
        hint: p.lilac,
        search: p.gold,
        selection: p.ice,
        punctuation: p.slate,
        hues: [p.ice, p.mint, p.aqua, p.gold, p.orchid, p.peach, p.lilac],
        syntax: [
            p.sage, p.peach, p.ice, p.lilac, p.aqua, p.orchid, p.peach, p.slate, p.rose, p.gold,
        ],
        modes: [p.ice, p.mint, p.lilac, p.amber],
        kinds,
    }
}
fn cyber(p: Cyber, light: bool) -> Source {
    let mut kinds = [p.fg; 26];
    for (kind, value) in [
        (1, p.green),
        (2, p.blue),
        (3, p.blue),
        (4, p.purple),
        (5, p.green),
        (6, p.orange),
        (7, p.blue),
        (8, p.blue),
        (9, p.blue),
        (10, p.red),
        (11, p.orange),
        (12, p.orange),
        (13, p.orange),
        (14, p.magenta),
        (15, p.green),
        (16, p.green),
        (17, p.green),
        (18, p.green),
        (19, p.green),
        (20, p.orange),
        (21, p.pink),
        (22, p.blue),
        (23, p.green),
        (24, p.magenta),
        (25, p.pink),
    ] {
        kinds[kind] = value;
    }
    Source {
        light,
        night: p.bg,
        glass_top: blend(p.bg_alt, p.fg, if light { 0.9 } else { 0.94 }),
        glass_bottom: p.bg,
        ink: [
            p.fg,
            blend(p.fg, p.grey, 0.55),
            p.grey,
            blend(p.grey, p.bg, 0.55),
        ],
        accent: p.cyan,
        error: p.red,
        success: p.green,
        warning: p.yellow,
        info: p.blue,
        hint: p.cyan,
        search: p.cyan,
        selection: p.blue,
        punctuation: p.fg,
        hues: [p.blue, p.green, p.cyan, p.yellow, p.magenta, p.orange, p.purple],
        syntax: [
            p.green, p.orange, p.blue, p.orange, p.purple, p.pink, p.cyan, p.purple, p.magenta,
            p.cyan,
        ],
        modes: [p.blue, p.green, p.purple, p.cyan],
        kinds,
    }
}
fn moon(c: &Config) -> Source {
    let m = |role: Moon| role.resolve(&c.theme);
    Source {
        light: false,
        night: m(Moon::Base),
        glass_top: m(Moon::Overlay),
        glass_bottom: m(Moon::Base),
        ink: [m(Moon::Text), m(Moon::Subtle), m(Moon::Muted), m(Moon::HighlightHigh)],
        accent: m(Moon::Iris),
        error: m(Moon::Love),
        success: m(Moon::Foam),
        warning: m(Moon::Gold),
        info: m(Moon::Foam),
        hint: m(Moon::Iris),
        search: m(Moon::Foam),
        selection: m(Moon::Iris),
        punctuation: m(Moon::Subtle),
        hues: [
            m(Moon::Pine),
            m(Moon::Foam),
            m(Moon::Foam),
            m(Moon::Gold),
            m(Moon::Iris),
            m(Moon::Gold),
            m(Moon::Iris),
        ],
        syntax: [
            m(Moon::Gold),
            m(Moon::Rose),
            m(Moon::Pine),
            m(Moon::Iris),
            m(Moon::Foam),
            m(Moon::Rose),
            m(Moon::Rose),
            m(Moon::Subtle),
            m(Moon::Iris),
            m(Moon::Love),
        ],
        modes: [m(Moon::Pine), m(Moon::Foam), m(Moon::Iris), m(Moon::Gold)],
        kinds: [m(Moon::Foam); 26],
    }
}
impl Theme {
    pub fn mode(&self, mode: crate::editor::Mode) -> Hsla {
        use crate::editor::Mode;
        self.modes[match mode {
            Mode::Normal => 0,
            Mode::Insert => 1,
            Mode::Visual => 2,
            _ => 3,
        }]
    }
    pub fn token(&self, token: Token) -> Hsla {
        match token {
            Token::Comment => self.muted,
            Token::String => self.string,
            Token::Number => self.number,
            Token::Constant => self.constant,
            Token::Boolean => self.boolean,
            Token::Keyword => self.keyword,
            Token::Type => self.type_color,
            Token::Function => self.function,
            Token::Operator => self.operator,
            Token::Statement => self.statement,
            Token::Preprocessor => self.preprocessor,
            Token::Parameter | Token::Property => self.text,
        }
    }
    pub fn hue(&self, hue: Hue) -> Hsla {
        self.hues[hue as usize]
    }
    pub fn completion_kind(&self, kind: usize) -> Hsla {
        self.kinds.get(kind).copied().unwrap_or(self.secondary)
    }
    pub fn diagnostic(&self, severity: u32) -> Hsla {
        match severity {
            1 => self.error,
            2 => self.warning,
            4 => self.hint,
            _ => self.info,
        }
    }
    pub fn new(c: &Config) -> Self {
        use crate::appearance::{ThemeName, Variant};
        let options = &c.appearance;
        let name = options.name.unwrap_or(match c.theme_name.as_str() {
            "cyberdream" => ThemeName::Cyberdream,
            "rose-pine-moon" => ThemeName::RosePineMoon,
            _ => ThemeName::Lucent,
        });
        let light = options.variant == Variant::Light;
        let s = match name {
            ThemeName::Lucent => lucent(Lucent::variant(options.variant), light),
            ThemeName::Cyberdream => cyber(Cyber::variant(options.variant), light),
            ThemeName::RosePineMoon => moon(c),
        };
        let color = |value: u32| -> Hsla {
            let mut color: Hsla = rgb(value).into();
            color.s *= options.saturation;
            color
        };
        let glass = if options.transparent {
            options.glass
        } else {
            Glass::Solid
        };
        let light = s.light;
        let ink = color(s.ink[0]);
        let white = gpui::white();
        let black = gpui::black();
        // Glass alpha by material. Frosted glass sits over an already blurred
        // world, so it can be thinner than clear glass for the same legibility.
        // The bake's legibility clamp guarantees contrast over bright worlds,
        // so baked glass can stay thin enough for the world's hue to come through.
        let (top_alpha, bottom_alpha) = match (glass, light) {
            (Glass::Frosted, false) => (0.42, 0.66),
            (Glass::Clear, false) => (0.62, 0.80),
            (Glass::Frosted, true) => (0.55, 0.74),
            (Glass::Clear, true) => (0.70, 0.84),
            (Glass::Solid, _) => (1., 1.),
        };
        let float = options.surfaces.popup_opacity;
        let syntax = s.syntax.map(color);
        let mut t = Self {
            light,
            glass,
            text: ink,
            secondary: color(s.ink[1]),
            muted: color(s.ink[2]),
            faint: color(s.ink[3]),
            accent: color(s.accent),
            base: color(s.night),
            world_tint: color(s.night).opacity(c.background.overlay_opacity),
            glass_top: color(s.glass_top).opacity(top_alpha),
            glass_bottom: color(s.glass_bottom).opacity(bottom_alpha),
            float_top: color(blend(s.glass_top, s.ink[0], if light { 1. } else { 0.97 }))
                .opacity(float),
            float_bottom: color(blend(s.glass_bottom, s.glass_top, 0.55))
                .opacity((float + 0.04).min(1.)),
            rim: if light {
                white.opacity(0.95)
            } else {
                white.opacity(0.42)
            },
            edge: if light {
                black.opacity(0.10)
            } else {
                white.opacity(0.09)
            },
            sheen: white.opacity(if light { 0.55 } else { 0.06 }),
            well: if light {
                black.opacity(0.06)
            } else {
                black.opacity(0.22)
            },
            well_edge: if light {
                black.opacity(0.06)
            } else {
                white.opacity(0.06)
            },
            lens_top: if light {
                white.opacity(0.95)
            } else {
                white.opacity(0.16)
            },
            lens_bottom: if light {
                white.opacity(0.75)
            } else {
                white.opacity(0.07)
            },
            lens_edge: if light {
                black.opacity(0.10)
            } else {
                white.opacity(0.20)
            },
            hover: ink.opacity(if light { 0.06 } else { 0.06 }),
            pressed: ink.opacity(if light { 0.10 } else { 0.10 }),
            hairline: ink.opacity(if light { 0.10 } else { 0.08 }),
            scrim: color(s.night).opacity(if light { 0.35 } else { 0.50 }),
            shadow: black.opacity(if light { 0.16 } else { 0.55 }),
            overlay: color(s.glass_top).opacity(float),
            popup_border: ink.opacity(0.14),
            popup_selection: ink.opacity(if light { 0.08 } else { 0.10 }),
            error: color(s.error),
            success: color(s.success),
            warning: color(s.warning),
            info: color(s.info),
            hint: color(s.hint),
            selection: color(s.selection).opacity(if light { 0.22 } else { 0.28 }),
            active_line: ink.opacity(if light { 0.045 } else { 0.035 }),
            gutter: color(s.ink[3]),
            cursor: ink,
            matched: color(s.accent),
            punctuation: color(s.punctuation),
            search: color(s.search),
            italic_comments: options.italic_comments,
            hues: s.hues.map(color),
            string: syntax[0],
            number: syntax[1],
            function: syntax[2],
            keyword: syntax[3],
            type_color: syntax[4],
            constant: syntax[5],
            boolean: syntax[6],
            operator: syntax[7],
            statement: syntax[8],
            preprocessor: syntax[9],
            kinds: s.kinds.map(color),
            modes: s.modes.map(color),
        };
        if glass == Glass::Solid {
            t.world_tint = t.base.opacity(0.);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(color: Hsla) -> u32 {
        let c: gpui::Rgba = color.into();
        ((c.r * 255.).round() as u32) << 16
            | ((c.g * 255.).round() as u32) << 8
            | (c.b * 255.).round() as u32
    }
    #[test]
    fn lucent_is_the_default_with_semantic_roles_and_materials() {
        let t = Theme::new(&Config::default());
        let p = Lucent::DARK;
        assert_eq!(hex(t.base), p.night);
        assert_eq!(hex(t.text), p.ink);
        assert_eq!(hex(t.accent), p.ice);
        assert_eq!(t.glass, Glass::Frosted);
        for (token, expected) in [
            (Token::String, p.sage),
            (Token::Function, p.ice),
            (Token::Type, p.aqua),
            (Token::Keyword, p.lilac),
            (Token::Statement, p.rose),
            (Token::Comment, p.ink3),
        ] {
            assert_eq!(hex(t.token(token)), expected);
        }
        assert_eq!(hex(t.diagnostic(1)), p.coral);
        assert_eq!(hex(t.diagnostic(2)), p.amber);
        for (mode, expected) in [
            (crate::editor::Mode::Normal, p.ice),
            (crate::editor::Mode::Insert, p.mint),
            (crate::editor::Mode::Visual, p.lilac),
            (crate::editor::Mode::Command, p.amber),
        ] {
            assert_eq!(hex(t.mode(mode)), expected);
        }
        // Ink is always opaque; materials are translucent and brighten upward.
        for ink in [t.text, t.secondary, t.muted, t.faint] {
            assert_eq!(ink.a, 1.);
        }
        assert!(t.glass_top.a < t.glass_bottom.a && t.glass_bottom.a < 1.);
        assert!(t.rim.a > t.edge.a && t.lens_top.a > t.lens_bottom.a);
        assert!(t.float_top.a >= 0.9 && t.hover.a < 0.1 && t.scrim.a > 0.3);
    }
    #[test]
    fn variants_glass_qualities_and_other_palettes() {
        let solid = Theme::new(&Config::parse("[theme]\ntransparent=false").unwrap());
        assert_eq!(solid.glass, Glass::Solid);
        assert_eq!((solid.glass_top.a, solid.glass_bottom.a), (1., 1.));
        assert_eq!(solid.world_tint.a, 0.);
        let clear = Theme::new(&Config::parse("[theme]\nglass='clear'").unwrap());
        let frosted = Theme::new(&Config::default());
        assert!(clear.glass_top.a > frosted.glass_top.a);
        let light = Theme::new(&Config::parse("[theme]\nvariant='light'").unwrap());
        assert!(light.light);
        assert_eq!(hex(light.text), Lucent::LIGHT.ink);
        let cyber = Theme::new(&Config::parse("[theme]\nname='cyberdream'").unwrap());
        assert_eq!(hex(cyber.base), Cyber::DARK.bg);
        assert_eq!(hex(cyber.token(Token::String)), Cyber::DARK.green);
        assert_eq!(hex(cyber.completion_kind(10)), Cyber::DARK.red);
        let legacy = Theme::new(&Config::parse("theme_name='cyberdream'").unwrap());
        assert_eq!(hex(legacy.base), Cyber::DARK.bg);
        let moon = Theme::new(&Config::parse("[theme]\nname='rose-pine-moon'").unwrap());
        assert_eq!(hex(moon.base), Moon::Base.rgb());
        assert_eq!(hex(moon.text), Moon::Text.rgb());
        assert_eq!(hex(moon.hue(Hue::Yellow)), Moon::Gold.rgb());
    }
}
