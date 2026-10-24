//! Event-driven visual overlays. Logical editing state never waits for animation.
use crate::{
    config::{Easing, Effects},
    editor::Mode,
    pixiedust::{Particle, Particles, graphics::CellSize},
    smear::{Point, Spring},
    theme::Theme,
};
use ratatui::{Frame, buffer::Buffer, layout::Rect};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn ease(t: f64, easing: Easing) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::Quadratic => 1.0 - (1.0 - t).powi(2),
        Easing::Cubic => 1.0 - (1.0 - t).powi(3),
    }
}
struct Scroll {
    from: f64,
    to: usize,
    start: Instant,
}
#[derive(Clone, Copy, Debug)]
pub struct CursorTarget {
    pub position: (u16, u16),
    pub viewport: Rect,
    pub origin: (usize, usize),
    pub logical: usize,
    pub revision: u64,
}
pub struct Visuals {
    pub effects: Effects,
    scrolls: BTreeMap<u64, Scroll>,
    cursor: Option<Spring>,
    context: Option<(u64, u64, u8, Rect)>,
    last: Option<CursorTarget>,
    previous_frame: Option<Instant>,
    alternate: bool,
    scene: Option<(Buffer, CursorTarget)>,
    particles: Option<Particles>,
    pixel_cell: Option<CellSize>,
}
impl Visuals {
    pub fn new(effects: Effects) -> Self {
        Self {
            effects,
            scrolls: BTreeMap::new(),
            cursor: None,
            context: None,
            last: None,
            previous_frame: None,
            alternate: false,
            scene: None,
            particles: None,
            pixel_cell: None,
        }
    }
    pub fn reset(&mut self) {
        self.scrolls.clear();
        self.cursor = None;
        self.last = None;
        self.context = None;
        self.previous_frame = None;
        self.scene = None;
        if let Some(p) = &mut self.particles {
            p.clear();
        }
    }
    pub fn pixel_cell(&mut self, cell: Option<CellSize>) -> bool {
        let cell = cell.and_then(|c| CellSize::new(c.width, c.height));
        if self.pixel_cell != cell {
            self.reset();
            self.pixel_cell = cell;
            true
        } else {
            false
        }
    }
    pub fn particles(&self) -> (&[Particle], [u16; 4]) {
        match (&self.particles, self.last, self.pixel_cell) {
            (Some(p), Some(t), Some(_)) => (
                p.particles(),
                [
                    t.viewport.x,
                    t.viewport.y,
                    t.viewport.width,
                    t.viewport.height,
                ],
            ),
            _ => (&[], [0; 4]),
        }
    }
    /// With an unchanged text scene, pixel sprites need no Ratatui frame/diff.
    pub fn tick_particles(&mut self, now: Instant) -> bool {
        if !self.effects.cursor
            || self.pixel_cell.is_none()
            || self.scrolling()
            || self.last.is_none()
        {
            return false;
        }
        let Some(particles) = &mut self.particles else {
            return false;
        };
        let dt = self
            .previous_frame
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        particles.advance(dt);
        self.previous_frame = Some(now);
        true
    }
    pub fn context(&mut self, view: u64, buffer: u64, mode: u8, area: Rect) {
        let context = (view, buffer, mode, area);
        if self.context != Some(context) {
            self.reset();
            self.context = Some(context);
        }
    }
    pub fn scroll(
        &mut self,
        view: u64,
        target: usize,
        row: usize,
        height: usize,
        now: Instant,
    ) -> usize {
        if !self.effects.scroll
            || height < 3
            || self.context.is_some_and(|c| c.2 != Mode::Normal as u8)
        {
            self.scrolls.remove(&view);
            return target;
        }
        let state = self.scrolls.entry(view).or_insert(Scroll {
            from: target as f64,
            to: target,
            start: now,
        });
        let progress = now.saturating_duration_since(state.start).as_secs_f64() * 1000.0
            / self.effects.scroll_ms as f64;
        let value =
            state.from + (state.to as f64 - state.from) * ease(progress, self.effects.easing);
        if state.to != target {
            state.from = value;
            state.to = target;
            state.start = now;
            if (target as f64 - value).abs() > height as f64 / 2.0 {
                state.from = target as f64;
            }
        }
        if !self.effects.scroll || height < 3 {
            state.from = target as f64;
            state.to = target;
            return target;
        }
        let progress = now.saturating_duration_since(state.start).as_secs_f64() * 1000.0
            / self.effects.scroll_ms as f64;
        let visual = (state.from
            + (target as f64 - state.from) * ease(progress, self.effects.easing))
        .round()
        .max(0.0) as usize;
        if progress >= 1.0 {
            state.from = target as f64;
        }
        visual.max((row + 1).saturating_sub(height)).min(row)
    }
    pub fn cursor(
        &mut self,
        target: Option<CursorTarget>,
        now: Instant,
        theme: &Theme,
        frame: &mut Frame,
    ) {
        let Some(target) = target else {
            self.cursor = None;
            self.last = None;
            self.previous_frame = None;
            if let Some(p) = &mut self.particles {
                p.clear();
            }
            return;
        };
        let normal = self.context.is_some_and(|c| c.2 == Mode::Normal as u8);
        if !self.effects.cursor || !normal {
            self.cursor = None;
            self.last = None;
            self.previous_frame = None;
            if let Some(p) = &mut self.particles {
                p.clear();
            }
            frame.set_cursor_position(target.position);
            return;
        }
        let point = Point {
            x: f64::from(target.position.0),
            y: f64::from(target.position.1),
        };
        if let Some(cell) = self.pixel_cell {
            let particles = self.particles.get_or_insert_with(Particles::default);
            let dt = self
                .previous_frame
                .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
            particles.advance(dt);
            if let Some(old) = self.last {
                if old.revision != target.revision
                    || old.viewport != target.viewport
                    || old.origin != target.origin
                {
                    particles.clear();
                } else if old.logical != target.logical {
                    let convert = |position: (u16, u16)| {
                        [
                            (f32::from(position.0) + 0.5) * f32::from(cell.width),
                            f32::from(position.1) * f32::from(cell.height),
                        ]
                    };
                    particles.emit(
                        convert(old.position),
                        convert(target.position),
                        f32::from(cell.height),
                        self.effects.pixiedust,
                    );
                }
            }
            self.last = Some(target);
            self.previous_frame = Some(now);
            self.cursor = None;
            frame.set_cursor_position(target.position);
            return;
        }
        let mut started = false;
        if let Some(previous) = self.last {
            if previous.revision != target.revision || previous.viewport != target.viewport {
                self.cursor = None;
            } else {
                let shift_x = previous.origin.0 as f64 - target.origin.0 as f64;
                let shift_y = previous.origin.1 as f64 - target.origin.1 as f64;
                if let Some(spring) = &mut self.cursor {
                    spring.shift(shift_x, shift_y);
                }
                // Only document navigation triggers a spring. A scrolling viewport
                // alone must never synthesize a cursor movement.
                if previous.logical != target.logical {
                    let old = Point {
                        x: (f64::from(previous.position.0) + shift_x).clamp(
                            f64::from(target.viewport.x),
                            f64::from(target.viewport.right().saturating_sub(1)),
                        ),
                        y: (f64::from(previous.position.1) + shift_y).clamp(
                            f64::from(target.viewport.y),
                            f64::from(target.viewport.bottom().saturating_sub(1)),
                        ),
                    };
                    let spring = self.cursor.get_or_insert_with(|| Spring::new(old));
                    started = !spring.active;
                    spring.retarget(point, self.effects.smear);
                } else if (shift_x != 0.0 || shift_y != 0.0)
                    && let Some(spring) = &mut self.cursor
                    && spring.active
                {
                    spring.retarget(point, self.effects.smear);
                }
            }
        }
        self.last = Some(target);
        let dt = if started {
            17.0
        } else {
            self.previous_frame.map_or(17.0, |t| {
                now.saturating_duration_since(t).as_secs_f64() * 1000.0
            })
        };
        self.previous_frame = Some(now);
        if let Some(spring) = &mut self.cursor {
            spring.step(dt, self.effects.smear);
            if spring.active {
                self.alternate = !self.alternate;
                spring.draw(
                    frame.buffer_mut(),
                    target.viewport,
                    self.effects.smear,
                    self.effects.smear.color.unwrap_or(theme.iris),
                    f64::from(self.effects.trail) / 10.0,
                    self.alternate,
                );
                // No set_cursor_position: Ratatui hides the native cursor for
                // exactly this frame. The next settled/non-Normal frame restores it.
                return;
            }
        }
        self.cursor = None;
        frame.set_cursor_position(target.position);
    }
    pub fn active(&self, _now: Instant) -> bool {
        // Keep one final frame scheduled to erase the last trail and settle scrolling.
        self.cursor.is_some()
            || self
                .particles
                .as_ref()
                .is_some_and(|p| !p.particles().is_empty())
            || self
                .scrolls
                .values()
                .any(|s| (s.from - s.to as f64).abs() > 0.01)
    }
    fn scrolling(&self) -> bool {
        self.scrolls
            .values()
            .any(|s| (s.from - s.to as f64).abs() > 0.01)
    }
    /// Save the underlay before painting the transient cursor. Input, worker
    /// results and scrolling still rebuild widgets; cursor-only frames reuse it.
    pub(crate) fn cache_scene(&mut self, buffer: &Buffer, target: Option<CursorTarget>) {
        if !self.effects.cursor
            || self.pixel_cell.is_some()
            || self.scrolling()
            || self.context.is_none_or(|c| c.2 != Mode::Normal as u8)
        {
            self.scene = None;
            return;
        }
        let Some(target) = target else {
            self.scene = None;
            return;
        };
        if let Some((scene, previous)) = &mut self.scene
            && scene.area == buffer.area
        {
            scene.content.clone_from_slice(&buffer.content);
            *previous = target;
        } else {
            self.scene = Some((buffer.clone(), target));
        }
    }
    /// Replay only when the caller knows the document/UI has not changed.
    pub fn redraw_cursor(&mut self, frame: &mut Frame, now: Instant, theme: &Theme) -> bool {
        if self.scrolling() {
            return false;
        }
        let Some((scene, target)) = &self.scene else {
            return false;
        };
        if scene.area != frame.area() {
            return false;
        }
        frame.buffer_mut().content.clone_from_slice(&scene.content);
        let target = *target;
        self.cursor(Some(target), now, theme, frame);
        true
    }
    pub fn frame_interval(&self) -> Duration {
        if self.pixel_cell.is_some() {
            return Duration::from_secs_f64(
                1.0 / f64::from(self.effects.pixiedust.fps.min(self.effects.fps)),
            );
        }
        self.effects.smear.frame_interval_ms.map_or_else(
            || Duration::from_secs_f64(1.0 / f64::from(self.effects.fps)),
            Duration::from_millis,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Preset};
    #[test]
    fn pixel_particles_preserve_cells_and_cancel_on_scroll_edit_mode_and_view_changes() {
        let mut visuals = Visuals::new(Config::default().effects());
        let area = Rect::new(0, 0, 80, 24);
        let now = Instant::now();
        visuals.pixel_cell(CellSize::new(10, 20));
        visuals.context(1, 1, Mode::Normal as u8, area);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut target = CursorTarget {
            position: (2, 2),
            viewport: area,
            origin: (0, 0),
            logical: 2,
            revision: 0,
        };
        let mut draw = |v: &mut Visuals, t: CursorTarget, at: Instant| {
            terminal
                .draw(|f| {
                    f.render_widget(
                        ratatui::widgets::Paragraph::new("underlying text remains intact"),
                        area,
                    );
                    v.cursor(Some(t), at, &Theme::default(), f);
                })
                .unwrap();
        };
        draw(&mut visuals, target, now);
        target.position.0 = 12;
        target.logical = 12;
        draw(&mut visuals, target, now + Duration::from_millis(10));
        assert!(visuals.active(now));
        assert!(!visuals.particles().0.is_empty());
        target.origin.1 = 1;
        draw(&mut visuals, target, now + Duration::from_millis(20));
        assert!(!visuals.active(now));
        target.position.0 = 22;
        target.logical = 22;
        draw(&mut visuals, target, now + Duration::from_millis(30));
        assert!(visuals.active(now));
        target.revision = 1;
        draw(&mut visuals, target, now + Duration::from_millis(40));
        assert!(!visuals.active(now));
        visuals.context(1, 1, Mode::Insert as u8, area);
        draw(&mut visuals, target, now + Duration::from_millis(50));
        assert!(!visuals.active(now));
        visuals.context(2, 1, Mode::Normal as u8, area);
        draw(&mut visuals, target, now + Duration::from_millis(60));
        assert!(!visuals.active(now));
        for (i, c) in "underlying text remains intact".chars().enumerate() {
            assert_eq!(
                terminal.backend().buffer()[(i as u16, 0)].symbol(),
                c.to_string()
            );
        }
    }
    #[test]
    fn pixel_disabled_has_no_particle_allocation_and_expiration_has_no_idle_work() {
        let config = Config::parse("[visual]\ncursor_effect='off'").unwrap();
        let mut v = Visuals::new(config.effects());
        let area = Rect::new(0, 0, 80, 24);
        v.pixel_cell(CellSize::new(10, 20));
        v.context(1, 1, Mode::Normal as u8, area);
        let now = Instant::now();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        for col in [1, 20, 30] {
            terminal
                .draw(|f| {
                    v.cursor(
                        Some(CursorTarget {
                            position: (col, 1),
                            viewport: area,
                            origin: (0, 0),
                            logical: col as usize,
                            revision: 0,
                        }),
                        now,
                        &Theme::default(),
                        f,
                    )
                })
                .unwrap();
        }
        assert!(v.particles.is_none());
        assert!(!v.active(now));
        v.effects = Config::default().effects();
        v.reset();
        v.context(1, 1, Mode::Normal as u8, area);
        for (col, ms) in [(1, 0), (20, 10), (20, 1000)] {
            terminal
                .draw(|f| {
                    v.cursor(
                        Some(CursorTarget {
                            position: (col, 1),
                            viewport: area,
                            origin: (0, 0),
                            logical: col as usize,
                            revision: 0,
                        }),
                        now + Duration::from_millis(ms),
                        &Theme::default(),
                        f,
                    )
                })
                .unwrap();
        }
        assert!(!v.active(now));
        assert!(v.particles().0.is_empty());
    }
    #[test]
    fn scroll_is_bounded_and_settles_without_changing_targets() {
        let c = Config {
            preset: Preset::Beautiful,
            ..Config::default()
        };
        let mut v = Visuals::new(c.effects());
        let now = Instant::now();
        assert_eq!(v.scroll(1, 0, 10, 20, now), 0);
        let top = v.scroll(1, 5, 15, 20, now);
        assert_eq!(top, 0);
        assert!(v.active(now));
        assert_eq!(v.scroll(1, 5, 15, 20, now + Duration::from_millis(200)), 5);
        assert!(!v.active(now + Duration::from_millis(200)));
        assert_eq!(
            v.scroll(1, 500, 510, 20, now + Duration::from_millis(201)),
            500
        );
    }
    #[test]
    fn minimal_has_no_animation_frames_and_overrides_are_independent() {
        let c =
            Config::parse("preset='minimal'\n[visual]\ncursor_animation=true\ntrail_intensity=3")
                .unwrap();
        assert!(c.effects().cursor);
        assert!(!c.effects().scroll);
        assert_eq!(c.effects().trail, 3);
        let c = Config {
            preset: Preset::Minimal,
            ..Config::default()
        };
        let v = Visuals::new(c.effects());
        assert!(!v.active(Instant::now()));
        assert!(!v.effects.cursor);
    }
}
