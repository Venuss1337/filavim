//! Four-corner cursor presentation; logical editing state remains in the editor.
//! Independently implemented from Neovide's leading/trailing-corner timing idea.
//!
//! Fixed-size geometry and delta-time simulation have no GPUI/protocol dependency.
//! The native `cursor_view` supplies target geometry, clipping and frame scheduling;
//! this state must never be used to decide where text edits occur.
use crate::config::CursorConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}
impl Point {
    fn mix(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
    fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Geometry {
    pub fn corners(self) -> [Point; 4] {
        [
            Point {
                x: self.x,
                y: self.y,
            },
            Point {
                x: self.x + self.width,
                y: self.y,
            },
            Point {
                x: self.x + self.width,
                y: self.y + self.height,
            },
            Point {
                x: self.x,
                y: self.y + self.height,
            },
        ]
    }
    fn center(self) -> Point {
        Point {
            x: self.x + self.width * 0.5,
            y: self.y + self.height * 0.5,
        }
    }
}
fn center(corners: [Point; 4]) -> Point {
    Point {
        x: corners.iter().map(|p| p.x).sum::<f32>() * 0.25,
        y: corners.iter().map(|p| p.y).sum::<f32>() * 0.25,
    }
}
fn convex(corners: [Point; 4]) -> bool {
    (0..4).all(|i| {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let c = corners[(i + 2) % 4];
        (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x) > 0.001
    })
}
fn ease(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}
#[derive(Default)]
pub struct Cursor {
    pub corners: [Point; 4],
    target: Option<Geometry>,
    start: [Point; 4],
    direction: Point,
    travel: f32,
    elapsed: f32,
    duration: f32,
    animating: bool,
    tracking: bool,
    velocity: [Point; 4],
}
impl Cursor {
    pub fn clear(&mut self) {
        self.target = None;
        self.animating = false;
        self.tracking = false;
        self.velocity = [Point::default(); 4];
    }
    pub fn active(&self) -> bool {
        self.animating
    }
    fn snap(&mut self, target: Geometry) {
        self.corners = target.corners();
        self.start = self.corners;
        self.target = Some(target);
        self.animating = false;
        self.tracking = false;
        self.velocity = [Point::default(); 4];
    }
    /// Continuous critically damped tracking; retargets retain velocity, not an animation queue.
    pub fn track(&mut self, target: Geometry, dt: f32, snap: bool, c: &CursorConfig) {
        if !c.active() || snap || !dt.is_finite() || dt > 0.18 || self.target.is_none() {
            self.snap(target);
            return;
        }
        if !self.tracking {
            self.velocity = [Point::default(); 4];
            self.tracking = true;
        }
        self.target = Some(target);
        let omega = 5.0 / (c.short_duration_ms as f32 / 1000.0);
        let dt = dt.max(0.0);
        let decay = (-omega * dt).exp();
        let spring = |position: f32, destination: f32, velocity: &mut f32| {
            let offset = position - destination;
            // Reversing typing/backspace must not carry momentum away from the new caret.
            if *velocity * offset > 0.0 {
                *velocity = 0.0;
            }
            let impulse = *velocity + omega * offset;
            let next = destination + (offset + impulse * dt) * decay;
            *velocity = (*velocity - omega * impulse * dt) * decay;
            if (next - destination) * offset < 0.0 {
                *velocity = 0.0;
                destination
            } else {
                next
            }
        };
        let mut settled = true;
        for (i, destination) in target.corners().iter().enumerate() {
            self.corners[i].x = spring(self.corners[i].x, destination.x, &mut self.velocity[i].x);
            self.corners[i].y = spring(self.corners[i].y, destination.y, &mut self.velocity[i].y);
            settled &= self.corners[i].distance(*destination) < 0.02
                && self.velocity[i].x.hypot(self.velocity[i].y) < 2.0;
        }
        self.animating = !settled;
        if settled || !convex(self.corners) {
            self.snap(target);
        }
    }
    pub fn step(
        &mut self,
        target: Geometry,
        dt: f32,
        snap: bool,
        c: &CursorConfig,
        cell_width: f32,
    ) {
        if !c.active() || snap || !dt.is_finite() || dt > 0.18 || self.target.is_none() {
            self.snap(target);
            return;
        }
        self.tracking = false;
        if self.target != Some(target) {
            let previous = self.target.unwrap_or(target);
            let origin = center(self.corners);
            let destination = target.center();
            self.travel = origin.distance(destination);
            self.direction = if self.travel > 0.001 {
                Point {
                    x: (destination.x - origin.x) / self.travel,
                    y: (destination.y - origin.y) / self.travel,
                }
            } else {
                Point::default()
            };
            self.start = self.corners;
            self.target = Some(target);
            self.elapsed = 0.;
            self.animating = true;
            let short = (target.y - previous.y).abs() < 0.01
                && (target.x - previous.x).abs() <= cell_width.max(1.) * 2.01;
            let strength = if c.smear { c.trail_size } else { 0.0 };
            let short_ms = c.duration_ms.min(c.short_duration_ms) as f32;
            self.duration = if short {
                short_ms + (c.duration_ms as f32 - short_ms) * 0.35 * strength
            } else {
                c.duration_ms as f32
            } / 1000.0;
        }
        if !self.animating {
            return;
        }
        self.elapsed += dt.max(0.);
        if self.elapsed >= self.duration {
            self.snap(target);
            return;
        }
        let strength = if c.smear { c.trail_size } else { 0. };
        let rear = ease(self.elapsed / self.duration);
        let front = ease(self.elapsed / (self.duration * (1. - 0.9 * strength)));
        let progress = (front + rear) * 0.5;
        let stretch = (self.travel * (front - rear) * 0.5).min(target.height * 2.0);
        let destination = target.corners();
        let middle = target.center();
        let support =
            (self.direction.x.abs() * target.width + self.direction.y.abs() * target.height) * 0.5;
        // A shared directional stretch keeps the four edges connected. At the front
        // and rear, travel follows the fast and slow timing respectively; intermediate
        // corners blend by projection. Previous deformation decays on retargeting.
        for (i, to) in destination.iter().enumerate() {
            let base = self.start[i].mix(*to, progress);
            let alignment = if support > 0.001 {
                ((to.x - middle.x) * self.direction.x + (to.y - middle.y) * self.direction.y)
                    / support
            } else {
                0.
            };
            self.corners[i] = Point {
                x: base.x + self.direction.x * stretch * alignment,
                y: base.y + self.direction.y * stretch * alignment,
            };
        }
        // Reversals and extreme glyph aspect changes must never create a folded path.
        if !convex(self.corners) {
            self.snap(target);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn at(x: f32, y: f32) -> Geometry {
        Geometry {
            x,
            y,
            width: 10.,
            height: 26.,
        }
    }
    #[test]
    fn leading_edges_advance_first_and_all_directions_settle_exactly() {
        for (x, y) in [
            (10., 0.),
            (-10., 0.),
            (0., 26.),
            (0., -26.),
            (30., 52.),
            (-30., 52.),
        ] {
            let c = CursorConfig::default();
            let mut cursor = Cursor::default();
            cursor.step(at(0., 0.), 0., false, &c, 10.);
            cursor.step(at(x, y), 0.016, false, &c, 10.);
            assert!(cursor.active());
            assert!(convex(cursor.corners));
            if x == 10. {
                assert!(cursor.corners[1].x - 10. > cursor.corners[0].x);
            }
            if y == 26. {
                assert!(cursor.corners[2].y - 26. > cursor.corners[1].y);
            }
            cursor.step(at(x, y), 0.15, false, &c, 10.);
            assert_eq!(cursor.corners, at(x, y).corners());
            assert!(!cursor.active());
        }
    }
    #[test]
    fn retargets_are_continuous_convex_and_time_based() {
        let c = CursorConfig::default();
        let mut a = Cursor::default();
        a.step(at(0., 0.), 0., false, &c, 10.);
        for (x, y, width) in [
            (40., 26., 10.),
            (-20., 52., 2.),
            (30., -26., 20.),
            (0., 0., 10.),
        ] {
            let target = Geometry { width, ..at(x, y) };
            let before = a.corners;
            a.step(target, 0., false, &c, 10.);
            assert_eq!(a.corners, before);
            a.step(target, 0.016, false, &c, 10.);
            assert!(convex(a.corners));
        }
        let mut b = Cursor::default();
        let mut d = Cursor::default();
        for v in [&mut b, &mut d] {
            v.step(at(0., 0.), 0., false, &c, 10.);
            v.step(at(40., 26.), 0., false, &c, 10.);
        }
        b.step(at(40., 26.), 0.032, false, &c, 10.);
        d.step(at(40., 26.), 0.016, false, &c, 10.);
        d.step(at(40., 26.), 0.016, false, &c, 10.);
        assert_eq!(b.corners, d.corners);
        let c = CursorConfig { smear: false, ..c };
        let mut rigid = Cursor::default();
        rigid.step(at(0., 0.), 0., false, &c, 10.);
        rigid.step(at(30., 0.), 0.016, false, &c, 10.);
        assert!((rigid.corners[1].x - rigid.corners[0].x - 10.).abs() < 0.001);
    }
    #[test]
    fn short_bar_motion_jumps_disabled_and_suspended_motion() {
        let mut c = CursorConfig::default();
        let mut cursor = Cursor::default();
        let bar = Geometry {
            width: 2.,
            ..at(0., 0.)
        };
        let next = Geometry { x: 10., ..bar };
        cursor.step(bar, 0., false, &c, 10.);
        cursor.track(next, 0.04, false, &c);
        cursor.track(next, 0.08, false, &c);
        assert_eq!(cursor.corners, next.corners());
        assert!(!cursor.active());
        for (target, dt, snap) in [
            (at(1000., 1000.), 0.016, true),
            (at(1010., 1000.), 0.2, false),
            (at(1000., 1000.), 0.016, true),
        ] {
            cursor.step(target, dt, snap, &c, 10.);
            assert_eq!(cursor.corners, target.corners());
            assert!(!cursor.active());
        }
        c.reduced_motion = true;
        cursor.step(at(0., 0.), 0.016, false, &c, 10.);
        assert!(!cursor.active());
        c.reduced_motion = false;
        c.enabled = false;
        cursor.step(at(10., 0.), 0.016, false, &c, 10.);
        assert!(!cursor.active());
    }
    #[test]
    fn line_commands_and_long_navigation_animate_to_exact_destination() {
        let c = CursorConfig::default();
        let mut cursor = Cursor::default();
        cursor.step(at(0., 0.), 0., false, &c, 10.);
        for target in [at(0., 7. * 26.), at(0., 15. * 26.), at(0., 1000.)] {
            cursor.step(target, 0.016, false, &c, 10.);
            assert!(cursor.active());
            assert_ne!(cursor.corners, target.corners());
            cursor.step(target, 0.15, false, &c, 10.);
            assert_eq!(cursor.corners, target.corners());
            assert!(!cursor.active());
        }
    }
    #[test]
    fn obsolete_settings_load_and_other_preferences_survive_persistence() {
        let text = "font_size=23\n[completion]\nauto_trigger=false\n[cursor]\npixiedust=true\nintensity=2.5\nduration_ms=70\nsmear=true\n";
        let c = crate::config::Config::parse(text).unwrap();
        assert_eq!(c.font_size, 23.);
        assert!(!c.completion.auto_trigger);
        assert_eq!(c.cursor.duration_ms, 70);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        let c = crate::setup::save_setting(&path, "cursor.trail_size", "0.8").unwrap();
        assert_eq!(c.cursor.trail_size, 0.8);
        assert_eq!(c.font_size, 23.);
        assert!(!c.completion.auto_trigger);
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(crate::setup::save_setting(&path, "cursor.pixiedust", "true").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(crate::config::Config::parse("[cursor]\ntrail_size=1.1").is_err());
        assert!(crate::config::Config::parse("[cursor]\ntrail_szie=0.5").is_err());
    }
}
