//! Presentation motion: retargetable springs on a monotonic clock.
//!
//! Lucent moves with three characters, each tuned for one job:
//! [`Character::Quick`] for direct feedback (hover, press), [`Character::Glide`]
//! for selections that travel (the tab lens, the command lens) and
//! [`Character::Settle`] for ambient light and values that follow state (mode light).
//!
//! Motion is presentation only. Owners sample a [`Motion`] while rendering and ask
//! for another frame only while [`Motion::active`] is true, so a settled interface
//! schedules nothing. Reduced motion snaps every value to its target. Nothing here
//! reads the wall clock implicitly: callers pass `now` in seconds from one
//! [`Clock`], which keeps tests deterministic.
use std::time::Instant;

/// Seconds since an owner-specific origin. One per entity is enough.
#[derive(Clone, Copy, Debug)]
pub struct Clock(Instant);
impl Default for Clock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock {
    pub fn now(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// Spring tuning: natural frequency (Hz) and damping ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Character {
    /// Hover and press feedback: fast, no overshoot (~90 ms to settle visually).
    Quick,
    /// A selection travelling to a new place: brisk with a trace of overshoot.
    Glide,
    /// Ambient light and values that follow state: slow and calm.
    Settle,
}
impl Character {
    fn tuning(self) -> (f64, f64) {
        match self {
            Self::Quick => (7.0, 1.0),
            Self::Glide => (3.6, 0.86),
            Self::Settle => (2.2, 1.0),
        }
    }
}

/// A damped spring toward a target, advanced analytically-stable in small steps.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    value: f64,
    velocity: f64,
    target: f64,
    at: f64,
    character: Character,
}
impl Motion {
    pub fn new(value: f64, character: Character) -> Self {
        Self {
            value,
            velocity: 0.,
            target: value,
            at: 0.,
            character,
        }
    }
    /// Retargets only when the target changes, so calling it every render keeps
    /// momentum. `reduced` places the value immediately.
    pub fn to(&mut self, target: f64, now: f64, reduced: bool) -> &mut Self {
        if reduced {
            self.jump(target, now);
        } else if target != self.target {
            self.advance(now);
            self.target = target;
            self.at = now;
        }
        self
    }
    /// Places the value immediately, discarding momentum.
    pub fn jump(&mut self, value: f64, now: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.;
        self.at = now;
    }
    pub fn target(&self) -> f64 {
        self.target
    }
    /// Samples the value at `now`. Time never runs backwards; a long stall (an
    /// inactive window, a debugger) settles instead of replaying the whole path.
    pub fn get(&mut self, now: f64) -> f64 {
        self.advance(now);
        self.value
    }
    /// Unit-clamped sample, for opacities and interpolation weights.
    pub fn unit(&mut self, now: f64) -> f32 {
        self.get(now).clamp(0., 1.) as f32
    }
    /// True while the value is visibly away from rest.
    pub fn active(&self) -> bool {
        (self.value - self.target).abs() > 1e-3 || self.velocity.abs() > 1e-3
    }
    fn advance(&mut self, now: f64) {
        let elapsed = now - self.at;
        self.at = self.at.max(now);
        if elapsed <= 0. || !self.active() {
            return;
        }
        if elapsed > 0.5 {
            self.value = self.target;
            self.velocity = 0.;
            return;
        }
        let (frequency, damping) = self.character.tuning();
        let omega = std::f64::consts::TAU * frequency;
        // Semi-implicit Euler in ≤4 ms steps: stable for these stiffnesses.
        let steps = (elapsed / 0.004).ceil().max(1.) as usize;
        let dt = elapsed / steps as f64;
        for _ in 0..steps {
            let force =
                -omega * omega * (self.value - self.target) - 2. * damping * omega * self.velocity;
            self.velocity += force * dt;
            self.value += self.velocity * dt;
        }
        if !self.active() {
            self.value = self.target;
            self.velocity = 0.;
        }
    }
}

/// Cubic ease-out, for one-shot timelines such as an entrance.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn springs_settle_exactly_and_report_activity() {
        for character in [Character::Quick, Character::Glide, Character::Settle] {
            let mut m = Motion::new(0., character);
            assert!(!m.active());
            m.to(1., 0., false);
            assert!(m.active());
            let mut t = 0.;
            let mut peak: f64 = 0.;
            while m.active() && t < 3. {
                t += 1. / 60.;
                peak = peak.max(m.get(t));
            }
            assert!(!m.active(), "{character:?} never settled");
            assert_eq!(m.get(t), 1.);
            // Quick and Settle never overshoot; Glide overshoots only slightly.
            match character {
                Character::Glide => assert!(peak < 1.04, "{peak}"),
                _ => assert!(peak <= 1.0 + 1e-9, "{character:?} {peak}"),
            }
        }
    }
    #[test]
    fn retargeting_keeps_momentum_reduced_motion_and_stalls_snap() {
        let mut m = Motion::new(0., Character::Glide);
        m.to(10., 0., false);
        let mid = m.get(0.05);
        assert!(mid > 0. && mid < 10.);
        // Repeating the same target must not restart the movement.
        m.to(10., 0.05, false);
        assert!(m.get(0.1) > mid);
        m.to(0., 0.1, true);
        assert_eq!(m.get(0.1), 0.);
        assert!(!m.active());
        m.to(5., 1., false);
        assert_eq!(m.get(10.), 5.);
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
    }
}
