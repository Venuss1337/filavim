//! FilaVim's native design system: layout tokens, type scale, elevation and the
//! small set of styled building blocks every interface is composed from.
//!
//! [DESIGN.md](../../DESIGN.md) explains the intent; this module is the contract.
//! Components return unattached GPUI elements: callers own ids, listeners, focus and
//! state, so the same button or row serves the editor, popups and the LSP manager.
//! Colors always come from [`Theme`] roles. Nothing here allocates per frame beyond
//! the element tree itself, schedules timers or animates.
use super::{FONT_FAMILY, icons, theme::Theme};
use gpui::{prelude::*, *};

/// Spacing steps on a 2/4 px grid. Prefer the named step to an ad-hoc literal.
pub mod space {
    pub const XXS: f32 = 2.;
    pub const XS: f32 = 4.;
    pub const SM: f32 = 6.;
    pub const MD: f32 = 8.;
    pub const LG: f32 = 12.;
    pub const XL: f32 = 16.;
    pub const XXL: f32 = 24.;
}
/// Corner radii by layer. Docked chrome is square; controls, floats and modals
/// round progressively so depth is legible from shape as well as shadow.
pub mod radius {
    pub const CONTROL: f32 = 4.;
    pub const FLOAT: f32 = 6.;
    pub const MODAL: f32 = 8.;
}

/// Type scale derived from the configured code font size. Code is always the
/// largest text; chrome steps down so the document stays the visual subject.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub code: f32,
    pub ui: f32,
    pub small: f32,
    pub micro: f32,
}
impl Scale {
    pub fn new(code: f32) -> Self {
        let ui = (code * 0.84).round().clamp(12., 17.);
        Self {
            code,
            ui,
            small: (ui - 2.).max(11.),
            micro: (ui - 3.).max(10.),
        }
    }
    /// Title/tab row; tall enough for 46 px native caption buttons.
    pub fn title_bar(self) -> f32 {
        (self.ui * 2.4).round()
    }
    pub fn status_bar(self) -> f32 {
        (self.ui * 1.9).round()
    }
    /// Interactive list row (explorer, pickers, manager catalog).
    pub fn row(self) -> f32 {
        (self.ui * 1.75).round()
    }
    pub fn control(self) -> f32 {
        (self.ui * 1.8).round()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Contextual and centered popups, toasts: above the editor, below dialogs.
    Floating,
    /// Decisions and managers: a scrim dims the application beneath.
    Modal,
}
pub fn shadow(theme: &Theme, level: Elevation) -> Vec<BoxShadow> {
    let (offset, blur) = match level {
        Elevation::Floating => (6., 18.),
        Elevation::Modal => (14., 40.),
    };
    vec![
        BoxShadow {
            color: theme.shadow,
            offset: point(px(0.), px(offset)),
            blur_radius: px(blur),
            spread_radius: px(-2.),
        },
        BoxShadow {
            color: theme.shadow.opacity(0.5),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        },
    ]
}
/// Smoked-glass surface: elevated fill, hairline edge, layer radius and shadow.
pub fn glass(theme: &Theme, level: Elevation) -> Div {
    div()
        .bg(theme.overlay)
        .border_1()
        .border_color(theme.popup_border.opacity(0.6))
        .rounded(px(match level {
            Elevation::Floating => radius::FLOAT,
            Elevation::Modal => radius::MODAL,
        }))
        .shadow(shadow(theme, level))
}
/// Full-window dimming layer beneath a modal. It never handles input itself.
pub fn scrim(theme: &Theme, viewport: Size<Pixels>) -> Div {
    div().w(viewport.width).h(viewport.height).bg(theme.scrim)
}

/// A glyph in a square, centered slot so mixed icons align in columns.
pub fn icon(glyph: &'static str, size: f32, color: Hsla) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px((size * 1.3).round()))
        .text_size(px(size))
        .text_color(color)
        .child(glyph)
}
/// The signal notch: a small glyph seated on the top edge of a minimal floating
/// surface, identifying its source (completion, documentation, diagnostics).
pub fn notch(theme: &Theme, glyph: &'static str, color: Hsla, size: f32) -> Div {
    let height = (size + 6.).round();
    div()
        .absolute()
        .left(px(space::LG))
        .top(px(-(height / 2.).round()))
        .h(px(height))
        .px(px(space::XS))
        .flex()
        .items_center()
        .rounded(px(height / 2.))
        .bg(theme.overlay.alpha(1.))
        .border_1()
        .border_color(theme.popup_border.opacity(0.6))
        .text_size(px(size))
        .line_height(px(height))
        .text_color(color)
        .child(glyph)
}

/// Uppercase section label for grouping related content inside a panel.
pub fn section(theme: &Theme, scale: Scale, text: &str) -> Div {
    div()
        .text_size(px(scale.micro))
        .text_color(theme.muted)
        .child(text.to_uppercase())
}
/// Tinted status pill. Use for state (mode, server status), not for actions.
pub fn badge(text: impl Into<SharedString>, color: Hsla, size: f32) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .px(px(space::SM))
        .rounded(px(radius::CONTROL))
        .bg(color.opacity(0.16))
        .text_color(color)
        .text_size(px(size))
        .child(text.into())
}
/// A key name drawn as a small physical key, for hints and shortcuts.
pub fn keycap(theme: &Theme, scale: Scale, key: impl Into<SharedString>) -> Div {
    let height = (scale.micro * 1.6).round();
    div()
        .flex_none()
        .h(px(height))
        .min_w(px(height))
        .px(px(space::XS))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::CONTROL))
        .border_1()
        .border_color(theme.hairline)
        .bg(theme.hover)
        .text_size(px(scale.micro))
        .line_height(px(height))
        .text_color(theme.secondary)
        .child(key.into())
}
/// Keyboard hint strip: `(keys, meaning)` pairs, quiet and left-aligned.
pub fn hints(theme: &Theme, scale: Scale, items: &[(&'static str, &'static str)]) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(space::LG))
        .gap_y(px(space::XS))
        .text_size(px(scale.micro))
        .text_color(theme.muted)
        .children(items.iter().map(|(keys, meaning)| {
            div()
                .flex()
                .items_center()
                .gap(px(space::XS))
                .child(keycap(theme, scale, *keys))
                .child(*meaning)
        }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The recommended or default action; at most one per group.
    Primary,
    /// Ordinary actions.
    Secondary,
    /// Low-emphasis toolbar actions.
    Ghost,
    /// Destroys or discards user data; never the default.
    Danger,
}
/// Text button. `focused` draws the keyboard focus ring; disabled buttons keep
/// their layout but lose hover, pointer and listeners (callers attach those).
pub fn button(
    theme: &Theme,
    scale: Scale,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    enabled: bool,
    focused: bool,
) -> Stateful<Div> {
    let (fill, text, edge) = match tone {
        Tone::Primary => (
            theme.accent.opacity(0.18),
            theme.accent,
            theme.accent.opacity(0.45),
        ),
        Tone::Secondary => (theme.hover, theme.text, theme.hairline),
        Tone::Ghost => (
            theme.hover.opacity(0.),
            theme.secondary,
            theme.hover.opacity(0.),
        ),
        Tone::Danger => (
            theme.error.opacity(0.12),
            theme.error,
            theme.error.opacity(0.4),
        ),
    };
    let hover = match tone {
        Tone::Primary => theme.accent.opacity(0.26),
        Tone::Danger => theme.error.opacity(0.2),
        _ => theme.pressed,
    };
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(space::SM))
        .h(px(scale.control()))
        .px(px(space::LG))
        .rounded(px(radius::CONTROL))
        .border_1()
        .text_size(px(scale.small))
        .whitespace_nowrap()
        .child(label.into())
        .map(|d| {
            if !enabled {
                d.bg(theme.hover.opacity(0.))
                    .border_color(theme.hairline.opacity(0.4))
                    .text_color(theme.muted)
            } else {
                d.bg(fill)
                    .border_color(if focused { theme.accent } else { edge })
                    .text_color(if tone == Tone::Ghost && focused {
                        theme.text
                    } else {
                        text
                    })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
            }
        })
}
/// Square icon-only control for chrome (sidebar toggle, close). Always pair with
/// a purpose the surrounding UI makes clear; there are no tooltips yet.
pub fn icon_button(
    theme: &Theme,
    id: impl Into<ElementId>,
    glyph: &'static str,
    size: f32,
    extent: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(extent))
        .rounded(px(radius::CONTROL))
        .text_size(px(size))
        .text_color(theme.secondary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.pressed).text_color(theme.text))
        .child(glyph)
}
/// Selectable list row with a rounded selection. Give its list `space::XS` side
/// padding so the selection reads as inset. Height and content are the caller's;
/// hover is suppressed on the selected row so state stays legible.
pub fn row(theme: &Theme, id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .px(px(space::MD))
        .flex()
        .items_center()
        .gap(px(space::SM))
        .rounded(px(radius::CONTROL))
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|d| {
            if selected {
                d.bg(theme.popup_selection)
            } else {
                d.hover(|s| s.bg(theme.hover).text_color(theme.text))
            }
        })
}
/// Horizontal hairline separator.
pub fn divider(theme: &Theme) -> Div {
    div().flex_none().h(px(1.)).w_full().bg(theme.hairline)
}
/// Panel heading: signal glyph, title and optional trailing meta text.
pub fn heading(
    theme: &Theme,
    scale: Scale,
    glyph: &'static str,
    color: Hsla,
    title: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .text_size(px(scale.ui))
        .text_color(theme.text)
        .child(icon(glyph, scale.ui, color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(title.into()),
        )
}
/// Root text setup shared by every independent overlay entity.
pub fn typeface(d: Div, theme: &Theme, size: f32) -> Div {
    d.font_family(FONT_FAMILY)
        .text_size(px(size))
        .text_color(theme.text)
}
/// The three states every severity-bearing surface distinguishes.
pub fn severity(theme: &Theme, severity: u32) -> (&'static str, Hsla) {
    match severity {
        1 => (icons::ERROR, theme.error),
        2 => (icons::WARNING, theme.warning),
        4 => (icons::HINT, theme.hint),
        _ => (icons::INFO, theme.info),
    }
}

/// Presentation-only path: drops the Windows verbatim prefix and shows paths
/// inside `root` relative to it. Never use the result for filesystem access.
pub fn display_path(path: &std::path::Path, root: &std::path::Path) -> String {
    fn plain(path: &std::path::Path) -> &std::path::Path {
        path.to_str()
            .and_then(|s| s.strip_prefix(r"\\?\"))
            .map_or(path, std::path::Path::new)
    }
    let path = plain(path);
    path.strip_prefix(plain(root))
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` exports its own `test` attribute; use the standard one.
    use std::prelude::v1::test;
    #[test]
    fn scale_keeps_code_largest_and_chrome_on_whole_pixels() {
        for code in [8., 12., 14., 18., 24., 48.] {
            let s = Scale::new(code);
            assert!(s.ui <= code.max(12.) && s.small < s.ui && s.micro < s.small);
            assert_eq!(s.title_bar().fract(), 0.);
            assert!(s.title_bar() >= 29. && s.row() > s.ui);
        }
        assert_eq!(Scale::new(18.).ui, 15.);
    }
    #[test]
    fn display_paths_are_relative_and_unprefixed() {
        use std::path::Path;
        let root = Path::new(r"C:\work\filavim");
        if cfg!(windows) {
            assert_eq!(
                display_path(Path::new(r"\\?\C:\work\filavim\src\main.rs"), root),
                r"src\main.rs"
            );
            assert_eq!(
                display_path(Path::new(r"\\?\D:\other.rs"), root),
                r"D:\other.rs"
            );
        }
        assert_eq!(
            display_path(Path::new("/tmp/x.rs"), Path::new("/tmp")),
            "x.rs"
        );
    }
}
