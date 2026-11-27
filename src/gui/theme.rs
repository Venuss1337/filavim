//! GPUI semantic colors resolved from shared palettes and appearance settings.
//!
//! [`crate::palette`] owns palette RGB values; [`crate::appearance`] owns portable
//! settings. Resolve [`Theme`] when settings change, not while painting each row.
use crate::{
    config::Config,
    palette::{Cyber, Moon, blend},
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
///
/// Interaction tints (`hover`, `pressed`) are derived from the foreground so they
/// read the same over opaque surfaces and translucent wallpaper. `hairline`
/// separates docked chrome; `scrim` and `shadow` belong to elevated layers only.
#[derive(Clone, Copy)]
pub struct Theme {
    pub hover: Hsla,
    pub pressed: Hsla,
    pub hairline: Hsla,
    pub scrim: Hsla,
    pub shadow: Hsla,
    pub light: bool,
    hues: [Hsla; 7],
    pub base: Hsla,
    pub surface: Hsla,
    pub chrome: Hsla,
    pub active_tab: Hsla,
    pub sidebar: Hsla,
    pub overlay: Hsla,
    pub editor: Hsla,
    pub muted: Hsla,
    pub text: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub popup_selection: Hsla,
    pub error: Hsla,
    pub success: Hsla,
    pub secondary: Hsla,
    pub border: Hsla,
    pub popup_border: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub hint: Hsla,
    pub active_line: Hsla,
    pub gutter: Hsla,
    pub cursor: Hsla,
    pub matched: Hsla,
    pub punctuation: Hsla,
    pub search: Hsla,
    pub italic_comments: bool,
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
        let options = &c.appearance;
        let color = |value: u32| -> Hsla {
            let mut color: Hsla = rgb(value).into();
            color.s *= options.saturation;
            color
        };
        let moon = |role: Moon| color(role.resolve(&c.theme));
        let cyber = options.name.map_or(c.theme_name == "cyberdream", |name| {
            name == crate::appearance::ThemeName::Cyberdream
        });
        let p = Cyber::variant(options.variant);
        // Derived interaction roles are filled in below, once text/base are known.
        let unset = Hsla::default();
        let mut t = if cyber {
            Self {
                hover: unset,
                pressed: unset,
                hairline: unset,
                scrim: unset,
                shadow: unset,
                light: options.variant == crate::appearance::Variant::Light,
                hues: [
                    color(p.blue),
                    color(p.green),
                    color(p.cyan),
                    color(p.yellow),
                    color(p.magenta),
                    color(p.orange),
                    color(p.purple),
                ],
                base: color(p.bg),
                surface: color(p.bg_alt),
                chrome: color(p.bg_alt),
                active_tab: color(p.bg_highlight),
                sidebar: color(p.bg_alt),
                // Floating layers sit one step above the editor plane.
                overlay: color(p.bg_alt),
                editor: color(p.bg),
                muted: color(p.grey),
                text: color(p.fg),
                accent: color(p.cyan),
                selection: color(p.bg_highlight),
                popup_selection: color(p.bg_highlight),
                error: color(p.red),
                success: color(p.green),
                secondary: color(p.grey),
                border: color(p.bg_highlight),
                popup_border: color(blend(p.bg_highlight, p.grey, 0.7)),
                warning: color(p.yellow),
                info: color(p.blue),
                hint: color(p.cyan),
                active_line: color(p.bg_highlight),
                gutter: color(blend(p.bg_highlight, p.fg, 0.9)),
                cursor: color(p.fg),
                matched: color(p.cyan),
                punctuation: color(p.fg),
                search: color(p.cyan),
                italic_comments: options.italic_comments,
                string: color(p.green),
                number: color(p.orange),
                function: color(p.blue),
                keyword: color(p.orange),
                type_color: color(p.purple),
                constant: color(p.pink),
                boolean: color(p.cyan),
                operator: color(p.purple),
                statement: color(p.magenta),
                preprocessor: color(p.cyan),
                kinds: [color(p.fg); 26],
                modes: [
                    color(p.blue),
                    color(p.green),
                    color(p.purple),
                    color(p.cyan),
                ],
            }
        } else {
            Self {
                hover: unset,
                pressed: unset,
                hairline: unset,
                scrim: unset,
                shadow: unset,
                light: false,
                hues: [
                    moon(Moon::Pine),
                    moon(Moon::Foam),
                    moon(Moon::Foam),
                    moon(Moon::Gold),
                    moon(Moon::Iris),
                    moon(Moon::Gold),
                    moon(Moon::Iris),
                ],
                base: moon(Moon::Base),
                surface: moon(Moon::Surface),
                chrome: moon(Moon::Surface),
                active_tab: moon(Moon::HighlightMed),
                sidebar: moon(Moon::Surface),
                overlay: moon(Moon::Overlay),
                editor: moon(Moon::Base),
                muted: moon(Moon::Muted),
                text: moon(Moon::Text),
                accent: moon(Moon::Iris),
                selection: moon(Moon::HighlightMed),
                popup_selection: moon(Moon::HighlightMed),
                error: moon(Moon::Love),
                success: moon(Moon::Foam),
                secondary: moon(Moon::Subtle),
                border: moon(Moon::HighlightHigh),
                popup_border: moon(Moon::HighlightHigh),
                warning: moon(Moon::Gold),
                info: moon(Moon::Foam),
                hint: moon(Moon::Iris),
                active_line: moon(Moon::HighlightLow),
                gutter: moon(Moon::Muted),
                cursor: moon(Moon::Text),
                matched: moon(Moon::Foam),
                punctuation: moon(Moon::Subtle),
                search: moon(Moon::Foam),
                italic_comments: options.italic_comments,
                string: moon(Moon::Gold),
                number: moon(Moon::Rose),
                function: moon(Moon::Pine),
                keyword: moon(Moon::Iris),
                type_color: moon(Moon::Foam),
                constant: moon(Moon::Rose),
                boolean: moon(Moon::Rose),
                operator: moon(Moon::Subtle),
                statement: moon(Moon::Iris),
                preprocessor: moon(Moon::Love),
                kinds: [moon(Moon::Foam); 26],
                modes: [
                    moon(Moon::Pine),
                    moon(Moon::Foam),
                    moon(Moon::Iris),
                    moon(Moon::Foam),
                ],
            }
        };
        if cyber {
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
                t.kinds[kind] = color(value);
            }
        }
        t.hover = t.text.opacity(if t.light { 0.07 } else { 0.06 });
        t.pressed = t.text.opacity(if t.light { 0.12 } else { 0.10 });
        t.hairline = t.border.opacity(if t.light { 0.9 } else { 0.7 });
        t.scrim = t.base.opacity(if t.light { 0.45 } else { 0.55 });
        t.shadow = gpui::black().opacity(if t.light { 0.16 } else { 0.45 });
        // State fills are translucent in every mode, so the cursor line, active
        // tab and list selection read the same over opaque or wallpaper surfaces.
        t.active_tab = t.active_tab.opacity(0.45);
        t.popup_selection = t.popup_selection.opacity(0.5);
        t.active_line = t.active_line.opacity(0.35);
        if options.transparent {
            // The root wallpaper already supplies the shared readability tint.
            t.chrome = t.chrome.opacity(0.);
            t.editor = t.base.opacity(options.surfaces.editor_opacity);
            t.overlay = t.overlay.opacity(options.surfaces.popup_opacity);
            t.sidebar = t.sidebar.opacity(options.surfaces.sidebar_opacity);
            // Keep large chrome quieter, but never fade its text or children.
            t.surface = t.surface.opacity(options.surfaces.popup_opacity);
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
    fn official_palettes_semantic_roles_and_transparency() {
        let t = Theme::new(&Config::default());
        assert_eq!(hex(t.base), Cyber::DARK.bg);
        assert_eq!(hex(t.text), Cyber::DARK.fg);
        for (token, expected) in [
            (Token::String, Cyber::DARK.green),
            (Token::Function, Cyber::DARK.blue),
            (Token::Type, Cyber::DARK.purple),
            (Token::Boolean, Cyber::DARK.cyan),
            (Token::Operator, Cyber::DARK.purple),
            (Token::Keyword, Cyber::DARK.orange),
            (Token::Constant, Cyber::DARK.pink),
            (Token::Statement, Cyber::DARK.magenta),
        ] {
            assert_eq!(hex(t.token(token)), expected);
        }
        assert_eq!(hex(t.diagnostic(1)), Cyber::DARK.red);
        assert_eq!(hex(t.diagnostic(2)), Cyber::DARK.yellow);
        assert_eq!(hex(t.diagnostic(3)), Cyber::DARK.blue);
        assert_eq!(hex(t.diagnostic(4)), Cyber::DARK.cyan);
        assert_eq!(hex(t.completion_kind(10)), Cyber::DARK.red);
        assert_eq!(
            hex(t.popup_border),
            blend(Cyber::DARK.bg_highlight, Cyber::DARK.grey, 0.7)
        );
        assert_eq!(t.text.a, 1.);
        assert_eq!(t.editor.a, 0.);
        assert_eq!(t.chrome.a, 0.);
        assert_eq!(t.sidebar.a, 0.);
        assert_eq!(t.active_tab.a, 0.45);
        assert_eq!(t.overlay.a, 0.92);
        assert_eq!(hex(t.overlay), Cyber::DARK.bg_alt);
        assert_eq!(hex(t.hue(Hue::Orange)), Cyber::DARK.orange);
        assert!(t.hover.a < 0.1 && t.scrim.a > 0.4 && !t.light);
        assert_eq!(t.popup_selection.a, 0.5);
        for (mode, expected) in [
            (crate::editor::Mode::Normal, Cyber::DARK.blue),
            (crate::editor::Mode::Insert, Cyber::DARK.green),
            (crate::editor::Mode::Visual, Cyber::DARK.purple),
            (crate::editor::Mode::Command, Cyber::DARK.cyan),
        ] {
            assert_eq!(hex(t.mode(mode)), expected);
        }
        for (variant, expected) in [("light", Cyber::LIGHT), ("muted", Cyber::MUTED)] {
            let config =
                Config::parse(&format!("[theme]\nvariant='{variant}'\ntransparent=false")).unwrap();
            let t = Theme::new(&config);
            assert_eq!(hex(t.base), expected.bg);
            assert_eq!(t.light, variant == "light");
            assert_eq!(hex(t.token(Token::String)), expected.green);
            assert_eq!(t.editor.a, 1.);
            assert_eq!(t.chrome.a, 1.);
            // Opaque surfaces stay opaque; state fills remain translucent tints.
            assert_eq!(t.overlay.a, 1.);
            assert_eq!((t.active_tab.a, t.active_line.a), (0.45, 0.35));
        }
        let t = Theme::new(&Config::parse("[theme]\nname='rose-pine-moon'").unwrap());
        assert_eq!(hex(t.base), Moon::Base.rgb());
        assert_eq!(hex(t.text), Moon::Text.rgb());
    }
}
