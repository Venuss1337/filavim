//! Window overlay with one owned frame timer; notification frames never notify EditorView.
use super::{
    design::{self, Elevation, Scale, space},
    icons,
    theme::Theme,
    toast::{Category, Manager, Notification},
};
use crate::config::Config;
use gpui::{prelude::*, *};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct ToastView {
    pub(super) manager: Manager,
    theme: Theme,
    scale: Scale,
    clock: Instant,
    frame: Option<Task<()>>,
}
impl ToastView {
    pub fn new(config: &Config) -> Self {
        Self {
            manager: Manager::new(config.cursor.reduced_motion),
            theme: Theme::new(config),
            scale: Scale::new(config.font_size),
            clock: Instant::now(),
            frame: None,
        }
    }
    pub fn configure(&mut self, config: &Config, cx: &mut Context<Self>) {
        self.theme = Theme::new(config);
        self.scale = Scale::new(config.font_size);
        self.manager.reduced_motion = config.cursor.reduced_motion;
        cx.notify();
    }
    /// Call on the UI thread, including from a GPUI entity update after async work.
    /// Returns None only when every visible slot is reserved by a persistent toast.
    pub fn show(&mut self, notification: Notification, cx: &mut Context<Self>) -> Option<u64> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            % 86400;
        let timestamp = format!(
            "{:02}:{:02}:{:02} UTC",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        );
        let id = self
            .manager
            .push(notification, self.clock.elapsed(), timestamp.into());
        cx.notify();
        id
    }
    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.manager.dismiss(id, self.clock.elapsed());
        self.schedule(cx);
        cx.notify();
    }
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if !self.manager.animating(self.clock.elapsed()) {
            self.frame = None;
        } else if self.frame.is_none() {
            let executor = cx.background_executor().clone();
            let delay = Duration::from_millis(if self.manager.reduced_motion { 33 } else { 16 });
            self.frame = Some(cx.spawn(async move |view, cx| {
                executor.timer(delay).await;
                let _ = view.update(cx, |v, cx| {
                    v.frame = None;
                    cx.notify();
                });
            }));
        }
    }
}
impl Render for ToastView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.clock.elapsed();
        self.manager.tick(now);
        self.schedule(cx);
        let theme = self.theme;
        let scale = self.scale;
        // Inside the editor pane's upper trailing corner, below the capsule row.
        let shell = design::Shell::new(
            window.viewport_size(),
            scale,
            window.is_maximized() || window.is_fullscreen(),
            None,
        );
        let top = f32::from(shell.slab.top()) + space::MD;
        let right = shell.margin + space::MD;
        let width = (f32::from(window.viewport_size().width) - right * 2.).clamp(0., 360.);
        let height = (f32::from(window.viewport_size().height) - top - space::LG).max(0.);
        let mut stack = design::typeface(div(), &theme, scale.ui)
            .absolute()
            .top(px(top))
            .right(px(right))
            .w(px(width))
            .max_h(px(height))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            // Room for the card shadows inside the clipped stack.
            .px(px(space::XS))
            .pb(px(space::LG));
        for toast in &self.manager.entries {
            let id = toast.id;
            let visual = toast.frame(now, self.manager.reduced_motion);
            let (glyph, color) = match toast.notification.category {
                Category::Information => (icons::INFO, theme.info),
                Category::Success => (icons::CHECK, theme.success),
                Category::Warning => (icons::WARNING, theme.warning),
                Category::Error => (icons::ERROR, theme.error),
            };
            stack = stack.child(
                design::glass(&theme, Elevation::Floating)
                    .id(("toast", id))
                    .relative()
                    .left(px(visual.offset))
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .gap(px(space::SM))
                    .opacity(visual.opacity)
                    .overflow_hidden()
                    .pl(px(space::MD))
                    .pr(px(space::XS))
                    .pt(px(space::SM))
                    .pb(px(space::MD))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .on_hover(cx.listener(move |v, hovering, _, cx| {
                        v.manager.hover(id, *hovering, v.clock.elapsed());
                        v.schedule(cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .h(px(scale.control()))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(scale.control() - space::SM))
                                    .rounded(px((scale.control() - space::SM) / 2.))
                                    .bg(color.opacity(0.14))
                                    .border_1()
                                    .border_color(color.opacity(0.36))
                                    .child(design::icon(glyph, scale.small, color)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::SM))
                                    .h(px(scale.control()))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .font_weight(FontWeight::BOLD)
                                            .child(SharedString::new(
                                                toast.notification.title.clone(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(scale.micro))
                                            .text_color(theme.muted)
                                            .child(SharedString::new(toast.timestamp.clone())),
                                    )
                                    .child(
                                        design::icon_button(
                                            &theme,
                                            ("toast-dismiss", id),
                                            icons::CLOSE,
                                            scale.small,
                                            scale.control() - space::XS,
                                        )
                                        .on_click(
                                            cx.listener(move |v, _, _, cx| {
                                                cx.stop_propagation();
                                                v.dismiss(id, cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .max_h(px(scale.small * 1.4 * 4.))
                                    .overflow_hidden()
                                    .text_size(px(scale.small))
                                    .text_color(theme.secondary)
                                    .child(SharedString::new(toast.notification.message.clone())),
                            ),
                    )
                    .child(
                        // Remaining lifetime, drawn along the card's bottom edge.
                        div()
                            .absolute()
                            .bottom(px(0.))
                            .left(px(design::radius::FLOAT))
                            .right(px(design::radius::FLOAT))
                            .h(px(2.))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(visual.progress))
                                    .rounded(px(1.))
                                    .bg(color.opacity(0.75))
                                    .shadow(vec![BoxShadow {
                                        color: color.opacity(0.5),
                                        offset: point(px(0.), px(0.)),
                                        blur_radius: px(6.),
                                        spread_radius: px(0.),
                                    }]),
                            ),
                    ),
            );
        }
        // Above deferred language popups, without a full-window mouse hitbox.
        deferred(stack).with_priority(30)
    }
}
