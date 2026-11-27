//! The Lucent shell: everything the editor view lays out inside the slab.
//!
//! ```text
//!  world ┌──────────────────────── slab (stage material) ────────────────────────┐
//!        │ ◉ project ◧     ╭ well ─[ lens: main.rs ]─ lib.rs ─╮      ⟳ work  – □ × │ header
//!        │ ────────────────────────────────────────────────────────────────────── │
//!        │ FILES   │  12  fn main() {          ╭ : command lens ─────────╮        │
//!        │ › src   │  13      let x = 1;       ╰─────────────────────────╯        │ body
//!        │ explorer│  gutter │ code                                              │
//!        │ ────────────────────────────────────────────────────────────────────── │
//!        │ ● NORMAL  src/main.rs · message          ⌕ 2/9  ⊗ 0  ⚠ 1   Ln 12 Col 5 │ footer
//!        └────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The stage paints the slab, its rim and the mode light beneath this view using
//! the same [`design::Shell`]; this module only places chrome and content. Shell
//! motion (the tab lens and the command lens) uses [`super::super::motion`]
//! springs and requests frames only while they move.
use super::{
    super::{FONT_FAMILY, design::Elevation, icons},
    EditorView, Mode, Request, design, space,
};
use gpui::{prelude::*, *};

/// Tab well geometry. Agave is monospaced, so tab widths are computed rather than
/// measured, which lets the lens glide between known positions.
struct Tabs {
    control: f32,
    pad: f32,
    gap: f32,
    slot: f32,
    icon: f32,
    advance: f32,
}
impl Tabs {
    const MAX_CHARS: usize = 24;
    fn width(&self, label: &str) -> f32 {
        let chars = label.chars().count().clamp(4, Self::MAX_CHARS) as f32;
        (space::LG + self.icon + space::SM + chars * self.advance + space::SM + self.slot + 3.)
            .ceil()
    }
}

impl EditorView {
    fn tab_label(&self, id: u64) -> String {
        self.model
            .get(id)
            .and_then(|editor| editor.buffer.path.as_ref())
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".into(), |n| n.to_string_lossy().into_owned())
    }
    /// Header: mark (painted by the stage), project, explorer toggle, the tab
    /// well with its gliding lens, transient work and caption plates.
    pub(super) fn header(
        &mut self,
        shell: &design::Shell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let scale = self.scale();
        let font = window.text_system().resolve_font(&font(FONT_FAMILY));
        let advance = window
            .text_system()
            .advance(font, px(scale.ui), 'm')
            .map_or(scale.ui * 0.6, |s| f32::from(s.width));
        let control = scale.control();
        let geometry = Tabs {
            control,
            pad: 3.,
            gap: 2.,
            slot: control - 10.,
            icon: (scale.ui * 1.3).round(),
            advance,
        };
        let order = self.model.order.clone();
        let active_id = self.model.id();
        let widths: Vec<f32> = order
            .iter()
            .map(|id| geometry.width(&self.tab_label(*id)))
            .collect();
        let mut x = geometry.pad;
        let mut lens = (geometry.pad, widths.first().copied().unwrap_or(80.));
        for (id, width) in order.iter().zip(&widths) {
            if *id == active_id {
                lens = (x, *width);
            }
            x += width + geometry.gap;
        }
        let now = self.clock.now();
        let reduced = self.model.editor.config.cursor.reduced_motion;
        if !self.tab_lens_ready {
            self.tab_lens.0.jump(lens.0 as f64, now);
            self.tab_lens.1.jump(lens.1 as f64, now);
            self.tab_lens_ready = true;
        }
        let lens_x = self.tab_lens.0.to(lens.0 as f64, now, reduced).get(now) as f32;
        let lens_w = self.tab_lens.1.to(lens.1 as f64, now, reduced).get(now) as f32;
        if self.tab_lens.0.active() || self.tab_lens.1.active() {
            window.request_animation_frame();
        }
        let tabs = order
            .iter()
            .zip(widths)
            .map(|(id, width)| {
                let editor = self.model.get(*id).expect("live document");
                let dirty = editor.buffer.dirty();
                let label = self.tab_label(*id);
                let (glyph, hue) = icons::file(&label);
                let id = *id;
                let active = id == active_id;
                let group = SharedString::from(format!("tab-{id}"));
                let slot = geometry.slot;
                div()
                    .id(("tab", id))
                    .group(group.clone())
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::SM))
                    .w(px(width))
                    .h(px(control))
                    .pl(px(space::LG))
                    .pr(px(3.))
                    .rounded(px(control / 2.))
                    .cursor_pointer()
                    .text_color(if active { theme.text } else { theme.muted })
                    .when(!active, |d| {
                        d.hover(|s| s.bg(theme.hover).text_color(theme.secondary))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, _, w, cx| {
                            cx.stop_propagation();
                            v.model.switch(id);
                            v.explorer.focused = false;
                            v.focus(w, cx);
                            v.changed(cx);
                        }),
                    )
                    .child(design::icon(
                        glyph,
                        scale.ui,
                        match hue {
                            Some(hue) if active => theme.hue(hue),
                            Some(hue) => theme.hue(hue).opacity(0.55),
                            None => theme.muted,
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(label),
                    )
                    .child(
                        // Unsaved documents show a light; the close plate replaces it
                        // under the pointer, so a click always lands on a visible ×.
                        div()
                            .relative()
                            .flex_none()
                            .size(px(slot))
                            .when(dirty, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .size_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(design::light(theme.accent, 6.))
                                        .group_hover(group.clone(), |s| s.opacity(0.)),
                                )
                            })
                            .child(
                                design::icon_button(
                                    &theme,
                                    ("close", id),
                                    icons::CLOSE,
                                    scale.micro,
                                    slot,
                                )
                                .absolute()
                                .opacity(if active && !dirty { 0.8 } else { 0. })
                                .group_hover(group, |s| s.opacity(1.))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |v, _, w, cx| {
                                        cx.stop_propagation();
                                        v.close_document(id, w, cx);
                                    }),
                                ),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let well_corner = control / 2. + geometry.pad;
        let well = design::capsule(&theme, Elevation::Floating, well_corner)
            .id("document-tabs")
            .flex()
            .items_center()
            .gap(px(geometry.gap))
            .p(px(geometry.pad))
            .max_w_full()
            .overflow_x_scroll()
            .text_size(px(scale.ui))
            .child(
                design::lens(&theme, control / 2.)
                    .absolute()
                    .top(px(geometry.pad))
                    .left(px(lens_x))
                    .w(px(lens_w))
                    .h(px(control)),
            )
            .children(tabs);
        let project = self.explorer.root.file_name().map_or_else(
            || design::plain_path(&self.explorer.root),
            |n| n.to_string_lossy().into_owned(),
        );
        let identity = design::capsule(&theme, Elevation::Floating, shell.capsule / 2.)
            .flex()
            .flex_shrink()
            .min_w(px(0.))
            .items_center()
            .gap(px(space::XS))
            .h(px(shell.capsule))
            .pl(px(space::SM + 1.))
            .pr(px(space::XS))
            .child(design::mark(
                theme.mode(self.model.editor.mode),
                (shell.capsule - 16.).max(12.),
            ))
            .child(
                div()
                    .id("project-name")
                    .flex_shrink()
                    .min_w(px(0.))
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .rounded(px(design::radius::CONTROL))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(scale.ui))
                    .text_color(if self.explorer.focused {
                        theme.accent
                    } else {
                        theme.text
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            cx.stop_propagation();
                            v.project_picker(cx);
                            cx.notify();
                        }),
                    )
                    .child(project),
            )
            .child(
                design::icon_button(
                    &theme,
                    "toggle-explorer",
                    icons::SIDEBAR,
                    scale.small,
                    control - 4.,
                )
                .when(self.explorer.visible, |d| d.text_color(theme.secondary))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|v, _, w, cx| {
                    cx.stop_propagation();
                    v.toggle_explorer();
                    v.focus(w, cx);
                    cx.notify();
                })),
            );
        let leading = div()
            .flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(0.))
            .child(identity);
        let work = if let Some(status) = self.services.workspace_status() {
            Some(status)
        } else if self.busy || self.tool_busy {
            Some("Working…".into())
        } else {
            None
        };
        let caption = |id: &'static str, glyph: &'static str, area: WindowControlArea| {
            let close = area == WindowControlArea::Close;
            design::capsule(&theme, Elevation::Floating, shell.capsule / 2.)
                .id(id)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(shell.capsule))
                .text_size(px(scale.small))
                .text_color(theme.secondary)
                .window_control_area(area)
                .hover(move |s| {
                    if close {
                        s.bg(theme.error.opacity(0.9)).text_color(gpui::white())
                    } else {
                        s.bg(theme.lens_top).text_color(theme.text)
                    }
                })
                .child(glyph)
        };
        let maximize_glyph = if window.is_maximized() {
            icons::RESTORE
        } else {
            icons::MAXIMIZE
        };
        let captions: Vec<AnyElement> = if cfg!(target_os = "macos") {
            Vec::new()
        } else {
            let minimize = caption("caption-minimize", icons::MINIMIZE, WindowControlArea::Min);
            let maximize = caption("caption-maximize", maximize_glyph, WindowControlArea::Max);
            let close = caption("close-application", icons::CLOSE, WindowControlArea::Close);
            // Windows resolves these regions through WM_NCHITTEST; clicks are also
            // delivered to elements there, so listeners would act twice.
            #[cfg(not(windows))]
            let (minimize, maximize, close) = (
                minimize.on_click(|_, w, _| w.minimize_window()),
                maximize.on_click(|_, w, _| w.zoom_window()),
                close.on_click(cx.listener(|v, _, w, cx| {
                    cx.stop_propagation();
                    v.close(w, cx);
                })),
            );
            vec![
                minimize.into_any_element(),
                maximize.into_any_element(),
                close.into_any_element(),
            ]
        };
        let trailing = div()
            .flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(0.))
            .items_center()
            .justify_end()
            .gap(px(space::SM))
            .when_some(work, |d, work| {
                d.child(
                    design::capsule(&theme, Elevation::Floating, shell.capsule / 2.)
                        .flex()
                        .flex_shrink()
                        .min_w(px(0.))
                        .items_center()
                        .gap(px(space::XS))
                        .h(px(shell.capsule))
                        .px(px(space::LG))
                        .overflow_hidden()
                        .text_size(px(scale.small))
                        .text_color(theme.secondary)
                        .child(design::icon(icons::SYNC, scale.small, theme.accent))
                        .child(
                            div()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(work),
                        ),
                )
            })
            .children(captions);
        // Center the well when it fits; otherwise it shrinks and scrolls.
        let center = div()
            .flex()
            .flex_shrink()
            .min_w(px(0.))
            .max_w(shell.top.size.width * 0.6)
            .px(px(space::MD))
            .child(well);
        div()
            .id("title-bar")
            .absolute()
            .left(shell.top.left())
            .top(shell.top.top())
            .w(shell.top.size.width)
            .h(shell.top.size.height)
            .flex()
            .items_center()
            .child(leading)
            .child(center)
            .child(trailing)
            .into_any_element()
    }

    /// Footer instrument strip: mode, document identity and feedback on the left;
    /// search, diagnostics and position on the right. Empty segments are omitted.
    pub(super) fn footer(&self, shell: &design::Shell) -> AnyElement {
        let theme = self.theme;
        let scale = self.scale();
        let e = &self.model.editor;
        let mode = theme.mode(e.mode);
        let (directory, name) = match &e.buffer.path {
            Some(path) => {
                let shown = design::display_path(path, &self.explorer.root);
                match shown.rfind(std::path::is_separator) {
                    Some(at) => (shown[..=at].to_owned(), shown[at + 1..].to_owned()),
                    None => (String::new(), shown),
                }
            }
            None => (String::new(), "Untitled".to_owned()),
        };
        let (glyph, hue) = icons::file(&name);
        let mut counts = [0usize; 4];
        for (_, (_, _, items)) in
            self.services
                .diagnostics
                .iter()
                .filter(|((_, id), (revision, _, _))| {
                    *id == e.document_id && *revision == e.buffer.revision
                })
        {
            for d in items {
                counts[(d.severity.clamp(1, 4) - 1) as usize] += 1;
            }
        }
        let attached = counts.iter().any(|c| *c > 0) || self.services.completion_ready(&self.model);
        let search = if e.last_search.is_empty() {
            None
        } else if e.search_error {
            Some(("invalid pattern".to_owned(), theme.error))
        } else if e.search_revision != e.buffer.revision {
            Some(("updating…".to_owned(), theme.muted))
        } else {
            let position = e
                .search_ranges
                .binary_search_by_key(&e.buffer.cursor, |r| r.start)
                .ok()
                .map_or(0, |i| i + 1);
            Some((
                format!("{position}/{}", e.search_ranges.len()),
                theme.secondary,
            ))
        };
        let segment = || div().flex().flex_none().items_center().gap(px(space::XS));
        let count = |glyph: &'static str, n: usize, color: Hsla| {
            segment()
                .text_color(if n > 0 { color } else { theme.faint })
                .child(design::icon(
                    glyph,
                    scale.small,
                    if n > 0 { color } else { theme.faint },
                ))
                .child(n.to_string())
        };
        let separator = || div().flex_none().text_color(theme.faint).child("·");
        let message = (!e.message.is_empty() && e.mode != Mode::Command).then(|| {
            segment()
                .flex_shrink()
                .min_w(px(0.))
                .overflow_hidden()
                .text_color(if e.error { theme.error } else { theme.secondary })
                .when(e.error, |d| {
                    d.child(design::icon(icons::ERROR, scale.small, theme.error))
                })
                .child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(e.message.clone()),
                )
        });
        let badge_height = (shell.footer - space::LG).max(scale.small + 6.);
        div()
            .id("status-bar")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::LG))
            .h(px(shell.footer))
            .pl(px(space::MD + space::XXS))
            .pr(px(space::XL))
            .overflow_hidden()
            .text_size(px(scale.small))
            .text_color(theme.secondary)
            .child(
                design::badge(e.mode.label(), mode, scale.micro)
                    .h(px(badge_height))
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                segment()
                    .flex_shrink()
                    .min_w(px(0.))
                    .child(design::icon(
                        glyph,
                        scale.small,
                        hue.map_or(theme.secondary, |h| theme.hue(h)),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_shrink()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex_shrink()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(theme.muted)
                                    .child(directory),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_color(theme.text)
                                    .child(name),
                            ),
                    )
                    .when(e.buffer.dirty(), |d| {
                        d.child(
                            segment()
                                .ml(px(space::XS))
                                .text_color(theme.accent)
                                .child(design::light(theme.accent, 5.))
                                .child("modified"),
                        )
                    }),
            )
            .when_some(message, |d, message| d.child(separator()).child(message))
            .child(div().flex_1())
            .when_some(search, |d, (text, color)| {
                d.child(
                    segment()
                        .text_color(color)
                        .child(design::icon(icons::SEARCH, scale.small, color))
                        .child(text),
                )
            })
            .when(attached, |d| {
                d.child(
                    segment()
                        .gap(px(space::MD))
                        .child(count(icons::ERROR, counts[0], theme.error))
                        .child(count(icons::WARNING, counts[1], theme.warning))
                        .when(counts[2] + counts[3] > 0, |d| {
                            d.child(count(icons::INFO, counts[2] + counts[3], theme.info))
                        }),
                )
            })
            .child(
                segment()
                    .gap(px(space::SM))
                    .child(div().text_color(theme.muted).child("Ln"))
                    .child(
                        div()
                            .text_color(theme.text)
                            .child((e.buffer.row() + 1).to_string()),
                    )
                    .child(div().ml(px(space::XS)).text_color(theme.muted).child("Col"))
                    .child(
                        div()
                            .text_color(theme.text)
                            .child((e.buffer.column(e.config.tab_width) + 1).to_string()),
                    ),
            )
            .into_any_element()
    }

    /// The command lens: Command mode rises as a floating pill near the top of
    /// the editor. The command caret shares its geometry, so the cursor travels
    /// from the text up into it. Its height is one code line plus padding.
    pub(super) fn command_lens(
        &mut self,
        area: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = self.clock.now();
        let e = &self.model.editor;
        if e.mode != Mode::Command {
            self.command_presence.jump(0., now);
            return None;
        }
        let reduced = e.config.cursor.reduced_motion;
        let presence = self.command_presence.to(1., now, reduced).unit(now);
        if self.command_presence.active() {
            window.request_animation_frame();
        }
        let theme = self.theme;
        let scale = self.scale();
        let line = self.line_height();
        let height = line + space::LG * 2. - 4.;
        let width = (f32::from(area.width) - space::XXXL * 3.)
            .min(760.)
            .max(260.);
        let left = ((f32::from(area.width) - width) / 2.).max(space::MD);
        let searching = e.searching;
        let text = format!("{}{}", if searching { "/" } else { ":" }, e.command);
        let prepare = cx.entity();
        let paint = cx.entity();
        let signal = theme.mode(Mode::Command);
        Some(
            design::capsule(&theme, Elevation::Floating, height / 2.)
                .absolute()
                .top(px(space::XL + (1. - presence) * -10.))
                .left(px(left))
                .w(px(width))
                .h(px(height))
                .opacity(presence)
                .flex()
                .items_center()
                .gap(px(space::MD))
                .pl(px(space::SM))
                .pr(px(space::LG))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(height - space::MD * 1.5))
                        .rounded(px((height - space::MD * 1.5) / 2.))
                        .bg(signal.opacity(0.16))
                        .border_1()
                        .border_color(signal.opacity(0.4))
                        .text_size(px(scale.small))
                        .text_color(signal)
                        .child(if searching { icons::SEARCH } else { icons::TERMINAL }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(line))
                        .text_size(px(e.config.font_size))
                        .line_height(px(line))
                        .child(
                            canvas(
                                move |bounds, window, cx| {
                                    prepare.update(cx, |view, cx| {
                                        view.prepare_command(bounds, text, window, cx)
                                    })
                                },
                                move |_, (), window, cx| {
                                    paint.update(cx, |view, cx| view.paint_command(window, cx))
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .child(design::hints(&theme, scale, &[("↵", "run"), ("Esc", "cancel")])),
                )
                .into_any_element(),
        )
    }

    /// A floating key-hint strip for the built-in help toggle.
    pub(super) fn help_hud(&self) -> Option<AnyElement> {
        let e = &self.model.editor;
        if !e.help || e.mode == Mode::Command {
            return None;
        }
        let theme = self.theme;
        let scale = self.scale();
        Some(
            div()
                .absolute()
                .bottom(px(space::XL))
                .left(px(0.))
                .right(px(0.))
                .flex()
                .justify_center()
                .child(
                    design::glass(&theme, Elevation::Floating)
                        .px(px(space::LG))
                        .py(px(space::MD))
                        .max_w(px(820.))
                        .child(design::hints(
                            &theme,
                            scale,
                            &[
                                ("W A S D", "move"),
                                ("I", "insert"),
                                ("V", "visual"),
                                ("U", "undo"),
                                ("Ctrl R", "redo"),
                                (":e", "open"),
                                (":w", "save"),
                                (":q", "quit"),
                                ("Esc", "close help"),
                            ],
                        )),
                )
                .into_any_element(),
        )
    }

    /// The welcome: shown only over an untouched, unnamed, empty document. It
    /// takes no input, so typing or clicking simply begins editing beneath it.
    pub(super) fn welcome(&self) -> Option<AnyElement> {
        let e = &self.model.editor;
        if e.buffer.path.is_some()
            || e.buffer.text.len_chars() > 0
            || e.buffer.dirty()
            || self.model.order.len() > 1
            || e.mode == Mode::Insert
        {
            return None;
        }
        let theme = self.theme;
        let scale = self.scale();
        let project = design::plain_path(&self.explorer.root);
        let action = |keys: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(space::XXL))
                .h(px(scale.row()))
                .child(div().text_color(theme.secondary).child(label))
                .child(
                    div()
                        .flex()
                        .gap(px(space::XS))
                        .children(keys.split(' ').map(|key| design::keycap(&theme, scale, key))),
                )
        };
        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(scale.ui * 26.))
                        .child(
                            div()
                                .flex()
                                .items_end()
                                .gap(px(space::MD))
                                .child(
                                    div()
                                        .text_size(px(scale.display))
                                        .line_height(px(scale.display * 1.05))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text)
                                        .child("filavim"),
                                )
                                .child(
                                    div()
                                        .mb(px(scale.display * 0.18))
                                        .child(design::badge(
                                            concat!("v", env!("CARGO_PKG_VERSION")),
                                            theme.accent,
                                            scale.micro,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .mt(px(space::XS))
                                .text_size(px(scale.ui))
                                .text_color(theme.muted)
                                .child("Modal editing, lit from within."),
                        )
                        .child(div().h(px(space::XXL)))
                        .child(design::section(&theme, scale, "Begin"))
                        .child(div().h(px(space::SM)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .text_size(px(scale.ui))
                                .child(action("Space f f", "Find a file"))
                                .child(action("Space f g", "Search the project"))
                                .child(action("Space p", "Run a command"))
                                .child(action("Space e", "Explorer"))
                                .child(action("Space f p", "Switch project"))
                                .child(action("i", "Start writing"))
                                .child(action("F1", "Guide")),
                        )
                        .child(div().h(px(space::XL)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .text_size(px(scale.micro))
                                .text_color(theme.faint)
                                .child(design::icon(icons::FOLDER, scale.micro, theme.faint))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(project),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn explorer_panel(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let scale = self.scale();
        let row_height = scale.row();
        let count = self.explorer.rows.len();
        let body = if count == 0 {
            let (glyph, text) = if self.explorer.index.is_none() {
                (icons::SYNC, "Indexing project…")
            } else {
                (icons::FOLDER, "No files to show")
            };
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .px(px(space::MD))
                .py(px(space::MD))
                .text_size(px(scale.small))
                .text_color(theme.muted)
                .child(design::icon(glyph, scale.small, theme.muted))
                .child(text)
                .into_any_element()
        } else {
            let entity = cx.entity();
            uniform_list("explorer", count, move |range, _, cx| {
                entity.update(cx, |v, cx| {
                    range.map(|i| v.explorer_row(i, row_height, cx)).collect()
                })
            })
            .track_scroll(self.explorer_scroll.clone())
            .px(px(space::SM))
            .pb(px(space::SM))
            .flex_1()
            .into_any_element()
        };
        div()
            .relative()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .text_size(px(scale.ui))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(px(scale.row() + space::SM))
                    .px(px(space::LG + space::XS))
                    .child(design::section(&theme, scale, "Files"))
                    .when(count > 0, |d| {
                        d.child(
                            div()
                                .text_size(px(scale.micro))
                                .text_color(theme.faint)
                                .child(count.to_string()),
                        )
                    }),
            )
            .child(body)
            .child(
                div()
                    .id("explorer-resize")
                    .absolute()
                    .top(px(0.))
                    .right(px(-2.))
                    .w(px(5.))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .when(self.resizing, |d| d.bg(theme.accent.opacity(0.5)))
                    .hover(|s| s.bg(theme.accent.opacity(0.3)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            v.resizing = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn explorer_row(&self, i: usize, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let scale = self.scale();
        let row = &self.explorer.rows[i];
        let name = row.entry.path.file_name().map_or_else(
            || row.entry.label.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
        let active = self.model.editor.buffer.path.as_ref() == Some(&row.entry.path);
        let selected = self.explorer.focused && self.explorer.selected == i;
        let (glyph, color) = if row.entry.directory {
            (
                if row.expanded {
                    icons::FOLDER_OPEN
                } else {
                    icons::FOLDER
                },
                theme.muted,
            )
        } else {
            let (glyph, hue) = icons::file(&name);
            (glyph, hue.map_or(theme.muted, |h| theme.hue(h)))
        };
        design::row(&theme, ("file", i), selected || active && !self.explorer.focused)
            .h(px(height))
            .pl(px(space::XS + row.depth as f32 * 14.))
            .gap(px(space::XS))
            .when(!(selected || active), |d| {
                d.text_color(if row.entry.directory {
                    theme.secondary
                } else {
                    theme.muted
                })
            })
            .child(design::icon(
                if !row.entry.directory {
                    ""
                } else if row.expanded {
                    icons::CHEVRON_DOWN
                } else {
                    icons::CHEVRON_RIGHT
                },
                scale.micro,
                theme.faint,
            ))
            .child(design::icon(glyph, scale.ui, color))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .when(active, |d| {
                d.child(design::light(self.theme.mode(self.model.editor.mode), 5.))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, _, w, cx| {
                    v.explorer.focused = true;
                    v.focus(w, cx);
                    if let Some(path) = v.explorer.activate(i) {
                        v.explorer.focused = false;
                        v.request(
                            Request::Open {
                                path,
                                revision: v.model.editor.buffer.revision,
                            },
                            cx,
                        );
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let requested = self.explorer.visible.then_some(self.explorer.width);
        let shell = design::Shell::new(
            viewport,
            self.scale(),
            window.is_maximized() || window.is_fullscreen(),
            requested,
        );
        // The stage paints the pane materials from the same geometry.
        self.stage
            .update(cx, |stage, cx| stage.set_sidebar(requested, cx));
        let slab = shell.slab;
        let pre = cx.entity();
        let paint = cx.entity();
        let overlay = cx.entity();
        let explorer = shell.sidebar.map(|bounds| {
            div()
                .absolute()
                .left(bounds.left())
                .top(bounds.top())
                .w(bounds.size.width)
                .h(bounds.size.height)
                .child(self.explorer_panel(f32::from(bounds.size.width), cx))
        });
        let header = self.header(&shell, window, cx);
        let footer = self.footer(&shell);
        let area = size(
            slab.size.width,
            slab.size.height - px(shell.footer + 1.),
        );
        let command = self.command_lens(area, window, cx);
        let welcome = self.welcome();
        let help = self.help_hud();
        let notice = self.manager_notice(cx);
        let theme = self.theme;
        let panes: Vec<Bounds<Pixels>> = shell.panes().collect();
        let sidebar = shell.sidebar;
        div()
            .relative()
            .size_full()
            .text_color(theme.text)
            .font_family(FONT_FAMILY)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, event: &MouseDownEvent, window, cx| {
                    if v.confirmation.is_none()
                        && v.services.panel.is_some()
                        && !v.popup_bounds.iter().any(|b| b.contains(&event.position))
                    {
                        v.dismiss_popup();
                        cx.notify();
                    }
                    // Bare world (around and between the panes and capsules)
                    // moves the window; capsules stop propagation themselves.
                    if !panes.iter().any(|b| b.contains(&event.position)) {
                        if event.click_count == 2 {
                            window.zoom_window();
                        } else {
                            v.title_drag = Some(event.position);
                        }
                    }
                }),
            )
            .on_mouse_move(cx.listener(move |v, event: &MouseMoveEvent, window, cx| {
                if let Some(start) = v.title_drag {
                    if event.pressed_button != Some(MouseButton::Left) {
                        v.title_drag = None;
                    } else if (event.position.x - start.x).abs() > px(4.)
                        || (event.position.y - start.y).abs() > px(4.)
                    {
                        v.title_drag = None;
                        super::super::application::drag_window(window);
                    }
                }
                if v.resizing
                    && let Some(sidebar) = sidebar
                {
                    v.explorer.width =
                        f32::from(event.position.x - sidebar.left()).clamp(160., 600.);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    if v.resizing {
                        cx.notify();
                    }
                    v.resizing = false;
                    v.title_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    if v.resizing {
                        cx.notify();
                    }
                    v.resizing = false;
                    v.title_drag = None;
                }),
            )
            .child(header)
            .children(explorer)
            .child(
                div()
                    .absolute()
                    .left(slab.left())
                    .top(slab.top())
                    .w(slab.size.width)
                    .h(slab.size.height)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .pt(px(space::MD))
                            .cursor(CursorStyle::IBeam)
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse))
                            .on_mouse_move(cx.listener(Self::mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|view, _, _, _| view.dragging = false),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|view, _, _, _| view.dragging = false),
                            )
                            .on_scroll_wheel(cx.listener(Self::wheel))
                            .child(
                                canvas(
                                    move |b, w, cx| {
                                        pre.update(cx, |view, cx| view.prepare(b, w, cx));
                                    },
                                    move |b, (), w, cx| {
                                        let focus = paint.read(cx).focus.clone();
                                        w.handle_input(
                                            &focus,
                                            ElementInputHandler::new(b, paint.clone()),
                                            cx,
                                        );
                                        paint.update(cx, |view, cx| view.paint(b, w, cx));
                                    },
                                )
                                .size_full(),
                            )
                            .children(welcome)
                            .children(notice)
                            .children(help)
                            .children(command),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h(px(1.))
                            .mx(px(space::XL))
                            .bg(theme.hairline),
                    )
                    .child(footer),
            )
            .child(
                canvas(
                    move |_, window, cx| {
                        let draws = overlay.update(cx, |view, cx| view.popups(window, cx));
                        for (mut element, origin) in draws {
                            let viewport = window.viewport_size();
                            element.layout_as_root(
                                size(
                                    AvailableSpace::Definite(viewport.width),
                                    AvailableSpace::Definite(viewport.height),
                                ),
                                window,
                                cx,
                            );
                            window.defer_draw(element, origin, 20);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full()
                .top(px(0.))
                .left(px(0.)),
            )
    }
}
