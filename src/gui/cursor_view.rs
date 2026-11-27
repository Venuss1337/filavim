//! Native-only cursor scene. Animation invalidates this child entity, not editor layouts.
use super::cursor::{Cursor, Geometry};
use crate::{config::CursorConfig, editor::Mode};
use gpui::{prelude::*, *};
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq)]
pub struct Scene {
    pub document: u64,
    pub top: usize,
    pub left: usize,
    pub bounds: Bounds<Pixels>,
    pub mode: Mode,
    pub scale: f32,
    pub font_size: f32,
    pub cell_width: f32,
}
impl Scene {
    fn can_animate_from(&self, previous: &Self) -> bool {
        self.document == previous.document
            && self.bounds == previous.bounds
            && self.scale == previous.scale
            && self.font_size == previous.font_size
            && self.cell_width == previous.cell_width
    }
}
pub struct CursorView {
    simulation: Cursor,
    target: Option<Geometry>,
    scene: Option<Scene>,
    config: CursorConfig,
    color: Hsla,
    clip: Bounds<Pixels>,
    bridge_clip: Option<Bounds<Pixels>>,
    surface_transition: bool,
    focus: FocusHandle,
    last: Option<Instant>,
    snap: bool,
    frame: Option<Task<()>>,
    activation: Option<Subscription>,
    slow_frames: u8,
    suspended_until: Option<Instant>,
    pub frames: u64,
}
impl CursorView {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            simulation: Cursor::default(),
            target: None,
            scene: None,
            config: CursorConfig::default(),
            color: Hsla::default(),
            clip: Bounds::default(),
            bridge_clip: None,
            surface_transition: false,
            focus,
            last: None,
            snap: true,
            frame: None,
            activation: None,
            slow_frames: 0,
            suspended_until: None,
            frames: 0,
        }
    }
    pub fn update_target(
        &mut self,
        target: Option<Geometry>,
        scene: Scene,
        c: &CursorConfig,
        color: Hsla,
        clip: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let changed = self.target != target
            || self.scene.as_ref() != Some(&scene)
            || self.config != *c
            || self.color != color
            || self.clip != clip;
        if !changed {
            return;
        }
        let compatible = self
            .scene
            .as_ref()
            .is_some_and(|previous| scene.can_animate_from(previous));
        let crossing = compatible
            && self.scene.as_ref().is_some_and(|previous| {
                (previous.mode == Mode::Command) != (scene.mode == Mode::Command)
            });
        if crossing && self.target.is_some() && target.is_some() {
            self.bridge_clip = Some(self.clip.union(&clip));
            self.surface_transition = true;
        }
        self.snap |= !compatible || self.config != *c;
        if self.snap {
            self.frame = None;
        }
        // Elapsed idle time is not an animation stall. Start a new movement at t=0.
        if self.target != target && !self.simulation.active() {
            self.last = Some(Instant::now());
        }
        self.target = target;
        self.scene = Some(scene);
        self.config = c.clone();
        self.color = color;
        self.clip = clip;
        if !c.active() || target.is_none() {
            self.stop();
        }
        cx.notify();
    }
    fn pause(&mut self) {
        self.frame = None;
        self.last = None;
    }
    fn stop(&mut self) {
        self.pause();
        self.simulation.clear();
        self.bridge_clip = None;
        self.surface_transition = false;
        self.snap = true;
    }
    pub fn active(&self) -> bool {
        self.target.is_some() && (self.simulation.active() || self.frame.is_some())
    }
    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.frames += 1;
        let started = Instant::now();
        if !window.is_window_active() || !self.focus.is_focused(window) {
            self.stop();
            return;
        }
        let Some(target) = self.target else {
            self.stop();
            return;
        };
        let dt = self
            .last
            .replace(started)
            .map_or(0., |last| started.duration_since(last).as_secs_f32());
        let suspended = self.suspended_until.is_some_and(|until| started < until);
        let mut c = self.config.clone();
        if suspended
            || self.scene.as_ref().is_some_and(|s| s.mode == Mode::Insert)
                && !c.animate_in_insert_mode
        {
            c.enabled = false;
        }
        let cell_width = self.scene.as_ref().map_or(target.width, |s| s.cell_width);
        let input = self
            .scene
            .as_ref()
            .is_some_and(|s| matches!(s.mode, Mode::Insert | Mode::Command));
        if input && !self.surface_transition {
            self.simulation.track(target, dt, self.snap, &c);
        } else {
            self.simulation.step(target, dt, self.snap, &c, cell_width);
        }
        self.snap = false;
        window.with_content_mask(
            Some(ContentMask {
                bounds: self.bridge_clip.unwrap_or(self.clip),
            }),
            |window| {
                let corners = self.simulation.corners;
                let mut color = self.color;
                if self
                    .scene
                    .as_ref()
                    .is_some_and(|s| !matches!(s.mode, Mode::Insert | Mode::Command))
                {
                    color.a = if self.scene.as_ref().is_some_and(|s| s.mode == Mode::Normal) {
                        0.60
                    } else {
                        0.42
                    };
                }
                if corners[0].y == corners[1].y
                    && corners[1].x == corners[2].x
                    && corners[2].y == corners[3].y
                    && corners[3].x == corners[0].x
                {
                    // Static and axis-aligned movement use the allocation-free quad path.
                    window.paint_quad(fill(
                        Bounds::new(
                            point(px(corners[0].x), px(corners[0].y)),
                            size(
                                px(corners[1].x - corners[0].x),
                                px(corners[3].y - corners[0].y),
                            ),
                        ),
                        color,
                    ));
                } else {
                    // GPUI's public convex path is two triangles, with no general tessellator.
                    let mut path = Path::new(point(px(corners[0].x), px(corners[0].y)));
                    for p in &corners[1..] {
                        path.line_to(point(px(p.x), px(p.y)));
                    }
                    window.paint_path(path, color);
                }
            },
        );
        if !self.simulation.active() {
            self.bridge_clip = None;
            self.surface_transition = false;
        }
        // Guard CPU submission cost, not GPU time. A long stall also snaps in step().
        if started.elapsed() > Duration::from_millis(3) {
            self.slow_frames += 1;
        } else {
            self.slow_frames = 0;
        }
        let cleanup = self.slow_frames >= 3;
        if cleanup {
            self.suspended_until = Some(started + Duration::from_secs(1));
            self.simulation.clear();
            self.snap = true;
            self.slow_frames = 0;
        }
        if (cleanup || self.simulation.active()) && self.frame.is_none() {
            let executor = cx.background_executor().clone();
            let delay = Duration::from_secs_f64(1. / f64::from(self.config.fps));
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
impl Render for CursorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.activation.is_none() {
            self.activation = Some(cx.observe_window_activation(window, |v, _, cx| {
                v.stop();
                cx.notify();
            }));
        }
        let entity = cx.entity();
        canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                entity.update(cx, |v, cx| v.paint(window, cx));
            },
        )
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::Scene;
    use crate::editor::Mode;
    use gpui::{Bounds, point, px, size};
    #[test]
    fn commands_and_navigation_keep_animation_but_document_and_layout_changes_snap() {
        let original = Scene {
            document: 1,
            top: 0,
            left: 0,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.))),
            mode: Mode::Normal,
            scale: 1.,
            font_size: 18.,
            cell_width: 9.,
        };
        let mut next = original.clone();
        next.mode = Mode::Command;
        assert!(next.can_animate_from(&original));
        next.mode = Mode::Normal;
        next.top = 100;
        next.left = 30;
        assert!(next.can_animate_from(&original));
        next.document = 2;
        assert!(!next.can_animate_from(&original));
        next.document = 1;
        next.bounds.size.width = px(700.);
        assert!(!next.can_animate_from(&original));
    }
}
