//! Window overlay with one owned frame timer; notification frames never notify EditorView.
use super::{
    FONT_FAMILY,
    theme::Theme,
    toast::{Category, Manager, Notification},
};
use crate::config::Config;
use gpui::{prelude::*, *};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct ToastView {
    pub(super) manager: Manager,
    theme: Theme,
    font_size: f32,
    clock: Instant,
    frame: Option<Task<()>>,
}
impl ToastView {
    pub fn new(config: &Config) -> Self {
        Self {
            manager: Manager::new(config.cursor.reduced_motion),
            theme: Theme::new(config),
            font_size: config.font_size.clamp(12., 16.),
            clock: Instant::now(),
            frame: None,
        }
    }
    pub fn configure(&mut self, config: &Config, cx: &mut Context<Self>) {
        self.theme = Theme::new(config);
        self.font_size = config.font_size.clamp(12., 16.);
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
        let width = (f32::from(window.viewport_size().width) - 24.).clamp(0., 350.);
        let height = (f32::from(window.viewport_size().height) - 44.).max(0.);
        let mut stack = div()
            .absolute()
            .top(px(32.))
            .right(px(12.))
            .w(px(width))
            .max_h(px(height))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(8.))
            .font_family(FONT_FAMILY)
            .text_size(px(self.font_size));
        for toast in &self.manager.entries {
            let id = toast.id;
            let visual = toast.frame(now, self.manager.reduced_motion);
            let color = match toast.notification.category {
                Category::Information => self.theme.info,
                Category::Success => self.theme.success,
                Category::Warning => self.theme.warning,
                Category::Error => self.theme.error,
            };
            stack = stack.child(
                div()
                    .id(("toast", id))
                    .relative()
                    .left(px(visual.offset))
                    .w_full()
                    .flex_shrink_0()
                    .opacity(visual.opacity)
                    .rounded(px(2.))
                    .bg(self.theme.base.opacity(0.28))
                    .border_1()
                    .border_color(self.theme.popup_border.opacity(0.65))
                    .text_color(self.theme.text)
                    .px(px(10.))
                    .py(px(7.))
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
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(21.))
                            .child(
                                div()
                                    .text_color(color)
                                    .child(toast.notification.category.icon()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(color)
                                    .child(SharedString::new(toast.notification.title.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(self.theme.secondary)
                                    .child(SharedString::new(toast.timestamp.clone())),
                            )
                            .child(
                                div()
                                    .id(("toast-dismiss", id))
                                    .cursor_pointer()
                                    .px(px(3.))
                                    .text_color(self.theme.secondary)
                                    .child("×")
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        cx.stop_propagation();
                                        v.dismiss(id, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h(px(1.))
                            .my(px(5.))
                            .w(relative(visual.progress))
                            .bg(color.opacity(0.65)),
                    )
                    .child(
                        div()
                            .max_h(px(self.font_size * 5.))
                            .overflow_hidden()
                            .child(SharedString::new(toast.notification.message.clone())),
                    ),
            );
        }
        // Above deferred language popups, without a full-window mouse hitbox.
        deferred(stack).with_priority(30)
    }
}
