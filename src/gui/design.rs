//! Lucent: FilaVim's design system as code. Tokens, the type scale, materials and
//! the stateless components every interface is composed from.
//!
//! [DESIGN.md](../../DESIGN.md) explains the intent; this module is the contract.
//! Components return unattached GPUI elements: callers own ids, listeners, focus and
//! state, so one button or row serves the editor, popups and the LSP manager.
//! Colors always come from [`Theme`] roles. Nothing here schedules frames; motion
//! lives with the owner of the state that moves (see [`super::motion`]).
//!
//! The material vocabulary, from back to front:
//! - **Slab**: the main frosted pane, painted by [`super::stage`], not here.
//! - **Well**: a recess cut into glass (tab track, inputs, segmented tracks).
//! - **Lens**: a raised, brighter pane marking the current selection.
//! - **Plate**: a control resting on glass (buttons, keycaps, chips).
//! - **Glass**: a floating pane above the slab (popups, pickers, toasts, dialogs).
//!
//! Every raised surface is lit from above: [`rim`] draws a crescent on its top
//! edge that tapers away along the corners, the same key light the slab has.
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
    pub const XXXL: f32 = 32.;
}
/// Corner radii. Depth reads from shape: the deeper a surface floats, the softer
/// its corners. Pills (`height / 2`) are for controls that sit on glass.
pub mod radius {
    pub const INNER: f32 = 4.;
    pub const CONTROL: f32 = 7.;
    pub const FLOAT: f32 = 11.;
    pub const MODAL: f32 = 16.;
    pub const SLAB: f32 = 18.;
}

/// Type scale derived from the configured code font size. Code is the largest
/// running text; chrome steps down so the document stays the subject. `display`
/// is reserved for the few hero figures (the welcome wordmark, dialog subjects).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub code: f32,
    pub ui: f32,
    pub small: f32,
    pub micro: f32,
    pub display: f32,
}
impl Scale {
    pub fn new(code: f32) -> Self {
        let ui = (code * 0.84).round().clamp(12., 17.);
        Self {
            code,
            ui,
            small: (ui - 2.).max(11.),
            micro: (ui - 3.).max(10.),
            display: (code * 3.4).round().clamp(40., 96.),
        }
    }
    /// Slab header: mark, tab well, status and caption plates.
    pub fn title_bar(self) -> f32 {
        (self.ui * 2.8).round()
    }
    /// Slab footer: mode chip, document identity and position.
    pub fn status_bar(self) -> f32 {
        (self.ui * 2.2).round()
    }
    /// Interactive list row (explorer, pickers, manager catalog).
    pub fn row(self) -> f32 {
        (self.ui * 1.8).round()
    }
    pub fn control(self) -> f32 {
        (self.ui * 1.9).round()
    }
}

/// Window composition shared by the stage (which paints pane materials) and the
/// editor view (which lays chrome and content out on them). Both call this one
/// function so material and content can never disagree.
///
/// The window is a world with glass on it: a row of floating capsules along the
/// top (no title bar), an optional explorer pane and the editor pane, separated
/// by a gap of world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shell {
    /// The capsule row along the top, in window coordinates.
    pub top: Bounds<Pixels>,
    /// The explorer pane, when visible.
    pub sidebar: Option<Bounds<Pixels>>,
    /// The editor pane.
    pub slab: Bounds<Pixels>,
    /// World visible around the panes and between them.
    pub margin: f32,
    pub footer: f32,
    /// Capsule height in the top row.
    pub capsule: f32,
}
impl Shell {
    /// `docked` (maximized or fullscreen) narrows the frame of world; panes never
    /// touch the window edge, so they always read as objects in a place.
    /// `sidebar` is the requested explorer width; it is clamped so the editor
    /// pane keeps a usable width.
    pub fn new(viewport: Size<Pixels>, scale: Scale, docked: bool, sidebar: Option<f32>) -> Self {
        let margin = if docked { space::MD } else { space::LG };
        let gap = space::MD + space::XXS;
        let capsule = scale.control() + space::SM;
        let header = capsule + margin + space::MD;
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let height = (vh - header - margin).max(1.);
        let mut left = margin;
        let sidebar = sidebar.map(|width| {
            let width = width
                .min((vw - margin * 2. - gap - 280.).max(120.))
                .max(120.);
            let bounds = Bounds::new(point(px(margin), px(header)), size(px(width), px(height)));
            left += width + gap;
            bounds
        });
        Self {
            top: Bounds::new(
                point(px(margin), px(margin)),
                size(px((vw - margin * 2.).max(1.)), px(capsule)),
            ),
            sidebar,
            slab: Bounds::new(
                point(px(left), px(header)),
                size(px((vw - left - margin).max(1.)), px(height)),
            ),
            margin,
            footer: scale.status_bar(),
            capsule,
        }
    }
    /// Every glass pane the stage paints, the explorer first.
    pub fn panes(&self) -> impl Iterator<Item = Bounds<Pixels>> + '_ {
        self.sidebar
            .iter()
            .copied()
            .chain(std::iter::once(self.slab))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Contextual and centered popups, toasts: above the slab, below dialogs.
    Floating,
    /// Decisions and managers: a scrim dims the application beneath.
    Modal,
}
/// A soft ambient shadow plus a tight contact shadow, so glass reads as lifted
/// rather than outlined.
pub fn shadow(theme: &Theme, level: Elevation) -> Vec<BoxShadow> {
    let (offset, blur) = match level {
        Elevation::Floating => (10., 28.),
        Elevation::Modal => (24., 64.),
    };
    vec![
        BoxShadow {
            color: theme.shadow,
            offset: point(px(0.), px(offset)),
            blur_radius: px(blur),
            spread_radius: px(-4.),
        },
        BoxShadow {
            color: theme.shadow.opacity(0.6),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        },
    ]
}
/// The key light on a raised surface's top edge: a crescent that is full width
/// on the top border and tapers to nothing along the upper corners. Add it as the
/// first child of a surface with a 1 px border and the same `corner` radius.
pub fn rim(color: Hsla, corner: f32) -> Div {
    div()
        .absolute()
        .top(px(-1.))
        .left(px(-1.))
        .right(px(-1.))
        .h(px((corner * 2.).max(6.)))
        .rounded(px(corner))
        .border_t_1()
        .border_color(color)
}
/// A faint highlight just inside a lit top edge, fading downward.
pub fn sheen(theme: &Theme, corner: f32, height: f32) -> Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(height))
        .rounded_t(px((corner - 1.).max(0.)))
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.sheen, 0.),
            linear_color_stop(theme.sheen.opacity(0.), 1.),
        ))
}
/// Floating or modal glass: near-opaque vertical gradient, hairline outline,
/// lit rim, sheen and lifted shadow. Code never shows through it.
pub fn glass(theme: &Theme, level: Elevation) -> Div {
    let corner = match level {
        Elevation::Floating => radius::FLOAT,
        Elevation::Modal => radius::MODAL,
    };
    capsule(theme, level, corner)
}
/// [`glass`] with an explicit corner radius, for pills such as the command lens.
pub fn capsule(theme: &Theme, level: Elevation, corner: f32) -> Div {
    div()
        .relative()
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.float_top, 0.),
            linear_color_stop(theme.float_bottom, 1.),
        ))
        .border_1()
        .border_color(theme.edge)
        .rounded(px(corner))
        .shadow(shadow(theme, level))
        .child(sheen(theme, corner, 28.))
        .child(rim(theme.rim, corner))
}
/// A recess in glass: darker, unlit, with a faint lower-weight outline.
pub fn well(theme: &Theme, corner: f32) -> Div {
    div()
        .relative()
        .bg(theme.well)
        .border_1()
        .border_color(theme.well_edge)
        .rounded(px(corner))
}
/// The selection lens: a brighter raised pane with a lit rim and a small
/// contact shadow. Its children sit on top of it.
pub fn lens(theme: &Theme, corner: f32) -> Div {
    div()
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.lens_top, 0.),
            linear_color_stop(theme.lens_bottom, 1.),
        ))
        .border_1()
        .border_color(theme.lens_edge.opacity(0.6))
        .rounded(px(corner))
        .shadow(vec![BoxShadow {
            color: theme.shadow.opacity(0.5),
            offset: point(px(0.), px(1.)),
            blur_radius: px(4.),
            spread_radius: px(0.),
        }])
        .child(rim(theme.lens_edge, corner))
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
/// A small luminous dot: the status light used by chips, badges and the mode.
pub fn light(color: Hsla, size: f32) -> Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(size / 2.))
        .bg(color)
        .shadow(vec![BoxShadow {
            color: color.opacity(0.55),
            offset: point(px(0.), px(0.)),
            blur_radius: px(size * 1.6),
            spread_radius: px(0.),
        }])
}
/// The FilaVim mark: a small glass lens whose core is lit in `color` (the mode).
pub fn mark(color: Hsla, size: f32) -> Div {
    let core = (size * 0.36).round();
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size / 2.))
        .bg(linear_gradient(
            160.,
            linear_color_stop(super::stage::mix(color, gpui::white(), 0.45), 0.),
            linear_color_stop(super::stage::mix(color, gpui::black(), 0.35), 1.),
        ))
        .border_1()
        .border_color(gpui::white().opacity(0.5))
        .shadow(vec![BoxShadow {
            color: color.opacity(0.5),
            offset: point(px(0.), px(0.)),
            blur_radius: px(size),
            spread_radius: px(0.),
        }])
        .child(
            div()
                .size(px(core))
                .rounded(px(core / 2.))
                .bg(gpui::white().opacity(0.92)),
        )
}
/// The signal notch: a small lens seated on the top edge of a minimal float,
/// naming its source (completion, documentation, signature, diagnostics).
pub fn notch(theme: &Theme, glyph: &'static str, color: Hsla, size: f32) -> Div {
    let height = (size + 8.).round();
    div()
        .absolute()
        .left(px(space::LG))
        .top(px(-(height / 2.).round()))
        .h(px(height))
        .px(px(space::SM))
        .flex()
        .items_center()
        .rounded(px(height / 2.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.float_top.alpha(1.), 0.),
            linear_color_stop(theme.float_bottom.alpha(1.), 1.),
        ))
        .border_1()
        .border_color(theme.edge)
        .text_size(px(size))
        .line_height(px(height))
        .text_color(color)
        .child(rim(theme.rim, height / 2.))
        .child(glyph)
}

/// Uppercase section label: the quiet scaffolding between groups.
pub fn section(theme: &Theme, scale: Scale, text: &str) -> Div {
    div()
        .text_size(px(scale.micro))
        .text_color(theme.muted)
        .font_weight(FontWeight::BOLD)
        .child(text.to_uppercase())
}
/// State chip: a status light and a label on a faint tint. Use for state
/// (mode, server status, scope), not for actions.
pub fn badge(text: impl Into<SharedString>, color: Hsla, size: f32) -> Div {
    let height = (size * 1.7).round();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .h(px(height))
        .px(px(space::MD))
        .rounded(px(height / 2.))
        .bg(color.opacity(0.12))
        .border_1()
        .border_color(color.opacity(0.28))
        .text_color(color)
        .text_size(px(size))
        .child(light(color, 5.))
        .child(text.into())
}
/// A key name drawn as a small physical key, for hints and shortcuts.
pub fn keycap(theme: &Theme, scale: Scale, key: impl Into<SharedString>) -> Div {
    let height = (scale.micro * 1.7).round();
    div()
        .relative()
        .flex_none()
        .h(px(height))
        .min_w(px(height))
        .px(px(space::SM))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::INNER + 1.))
        .border_1()
        .border_color(theme.edge)
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.lens_top, 0.),
            linear_color_stop(theme.lens_bottom, 1.),
        ))
        .text_size(px(scale.micro))
        .line_height(px(height))
        .text_color(theme.secondary)
        .child(rim(theme.lens_edge, radius::INNER + 1.))
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
                .gap(px(space::SM))
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
    /// Low-emphasis toolbar actions: no plate until hovered.
    Ghost,
    /// Destroys or discards user data; never the default.
    Danger,
}
/// Pill button resting on glass. `focused` draws the keyboard focus ring;
/// disabled buttons keep their layout but lose hover, pointer and listeners
/// (callers attach those).
pub fn button(
    theme: &Theme,
    scale: Scale,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    enabled: bool,
    focused: bool,
) -> Stateful<Div> {
    let height = scale.control();
    let corner = height / 2.;
    let (top, bottom, edge, text) = match tone {
        Tone::Primary => (
            theme.accent.opacity(0.34),
            theme.accent.opacity(0.20),
            theme.accent.opacity(0.55),
            theme.text,
        ),
        Tone::Secondary => (theme.lens_top, theme.lens_bottom, theme.edge, theme.text),
        Tone::Ghost => (
            theme.hover.opacity(0.),
            theme.hover.opacity(0.),
            theme.hover.opacity(0.),
            theme.secondary,
        ),
        Tone::Danger => (
            theme.error.opacity(0.24),
            theme.error.opacity(0.14),
            theme.error.opacity(0.5),
            theme.error,
        ),
    };
    let hover = match tone {
        Tone::Primary => theme.accent.opacity(0.42),
        Tone::Danger => theme.error.opacity(0.3),
        _ => theme.pressed,
    };
    div()
        .id(id)
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(space::SM))
        .h(px(height))
        .px(px(space::LG + space::XS))
        .rounded(px(corner))
        .border_1()
        .text_size(px(scale.small))
        .whitespace_nowrap()
        .map(|d| {
            if !enabled {
                d.border_color(theme.hairline).text_color(theme.faint)
            } else {
                d.bg(linear_gradient(
                    180.,
                    linear_color_stop(top, 0.),
                    linear_color_stop(bottom, 1.),
                ))
                .border_color(if focused { theme.accent } else { edge })
                .when(tone != Tone::Ghost, |d| {
                    d.child(rim(
                        if tone == Tone::Primary {
                            theme.accent.opacity(0.9)
                        } else {
                            theme.lens_edge
                        },
                        corner,
                    ))
                })
                .when(focused, |d| {
                    d.shadow(vec![BoxShadow {
                        color: theme.accent.opacity(0.35),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(8.),
                        spread_radius: px(0.),
                    }])
                })
                .text_color(if tone == Tone::Ghost && focused {
                    theme.text
                } else {
                    text
                })
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
            }
        })
        .child(label.into())
}
/// Round icon-only plate for chrome (close, sidebar). Invisible until hovered,
/// so chrome stays quiet; pair with a purpose the surrounding UI makes clear.
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
        .rounded(px(extent / 2.))
        .text_size(px(size))
        .text_color(theme.muted)
        .cursor_pointer()
        .hover(|s| s.bg(theme.pressed).text_color(theme.text))
        .child(glyph)
}
/// Selectable list row. The selection is a lens; hover is a faint tint and is
/// suppressed on the selected row so state stays legible. Give its list
/// `space::XS` side padding so the lens reads as inset.
pub fn row(theme: &Theme, id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .w_full()
        .px(px(space::MD))
        .flex()
        .items_center()
        .gap(px(space::SM))
        .rounded(px(radius::CONTROL))
        .border_1()
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|d| {
            if selected {
                d.bg(linear_gradient(
                    180.,
                    linear_color_stop(theme.lens_top, 0.),
                    linear_color_stop(theme.lens_bottom, 1.),
                ))
                .border_color(theme.lens_edge.opacity(0.45))
                .text_color(theme.text)
                .child(rim(theme.lens_edge, radius::CONTROL))
            } else {
                d.border_color(theme.hover.opacity(0.))
                    .hover(|s| s.bg(theme.hover).text_color(theme.text))
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
                .font_weight(FontWeight::BOLD)
                .child(title.into()),
        )
}
/// Root text setup shared by every independent overlay entity.
pub fn typeface(d: Div, theme: &Theme, size: f32) -> Div {
    d.font_family(FONT_FAMILY)
        .text_size(px(size))
        .text_color(theme.text)
}
/// The states every severity-bearing surface distinguishes.
pub fn severity(theme: &Theme, severity: u32) -> (&'static str, Hsla) {
    match severity {
        1 => (icons::ERROR, theme.error),
        2 => (icons::WARNING, theme.warning),
        4 => (icons::HINT, theme.hint),
        _ => (icons::INFO, theme.info),
    }
}

fn unverbatim(path: &std::path::Path) -> &std::path::Path {
    path.to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .map_or(path, std::path::Path::new)
}
/// Presentation-only absolute path without the Windows verbatim (`\\?\`) prefix.
pub fn plain_path(path: &std::path::Path) -> String {
    unverbatim(path).display().to_string()
}
/// Presentation-only path: drops the Windows verbatim prefix and shows paths
/// inside `root` relative to it. Never use the result for filesystem access.
pub fn display_path(path: &std::path::Path, root: &std::path::Path) -> String {
    let path = unverbatim(path);
    path.strip_prefix(unverbatim(root))
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
            assert!(s.display > s.code.max(s.ui));
            assert_eq!(s.title_bar().fract(), 0.);
            assert!(s.title_bar() >= 33. && s.row() > s.ui);
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
            assert_eq!(plain_path(Path::new(r"\\?\C:\work")), r"C:\work");
        }
        assert_eq!(
            display_path(Path::new("/tmp/x.rs"), Path::new("/tmp")),
            "x.rs"
        );
    }
}
