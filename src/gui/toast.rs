//! Bounded notification lifetimes, independent of GPUI and wall-clock adjustments.
//! The native view supplies monotonic elapsed time; timestamps are display-only.
use std::{sync::Arc, time::Duration};

const ENTER: Duration = Duration::from_millis(180);
const EXIT: Duration = Duration::from_millis(220);
pub const LIMIT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Information,
    Success,
    Warning,
    Error,
}
impl Category {
    fn duration(self) -> Duration {
        Duration::from_secs(match self {
            Self::Success => 3,
            Self::Information => 4,
            Self::Warning => 5,
            Self::Error => 7,
        })
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Information => "i",
            Self::Success => "✓",
            Self::Warning => "!",
            Self::Error => "×",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub category: Category,
    pub title: Arc<str>,
    pub message: Arc<str>,
    /// `None` requires explicit dismissal. Ordinary notifications have a deadline.
    pub duration: Option<Duration>,
}
impl Notification {
    pub fn new(category: Category, title: &str, message: &str) -> Self {
        Self {
            category,
            title: title.chars().take(64).collect::<String>().into(),
            message: message.chars().take(600).collect::<String>().into(),
            duration: Some(category.duration()),
        }
    }
    pub fn persistent(mut self) -> Self {
        self.duration = None;
        self
    }
    pub fn for_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }
}

pub struct Toast {
    pub id: u64,
    pub notification: Notification,
    pub timestamp: Arc<str>,
    created: Duration,
    entered: Duration,
    last: Duration,
    remaining: Option<Duration>,
    hovered: bool,
    exiting: Option<Duration>,
}
pub struct Frame {
    pub opacity: f32,
    pub offset: f32,
    pub progress: f32,
}
fn fraction(elapsed: Duration, duration: Duration) -> f32 {
    (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
}
fn ease(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}
impl Toast {
    pub fn frame(&self, now: Duration, reduced_motion: bool) -> Frame {
        let entrance = if reduced_motion {
            1.
        } else {
            ease(fraction(now.saturating_sub(self.created), ENTER))
        };
        let exit = self.exiting.map_or(0., |at| {
            if reduced_motion {
                1.
            } else {
                ease(fraction(now.saturating_sub(at), EXIT))
            }
        });
        Frame {
            opacity: entrance * (1. - exit),
            offset: if reduced_motion {
                0.
            } else {
                12. * (1. - entrance) + 6. * exit
            },
            progress: if self.exiting.is_some() {
                0.
            } else {
                self.remaining
                    .zip(self.notification.duration)
                    .map_or(1., |(left, total)| {
                        if total.is_zero() {
                            0.
                        } else {
                            fraction(left, total)
                        }
                    })
            },
        }
    }
}

#[derive(Default)]
pub struct Manager {
    pub entries: Vec<Toast>,
    serial: u64,
    pub reduced_motion: bool,
}
impl Manager {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            reduced_motion,
            ..Default::default()
        }
    }
    /// Newest first. Replace the oldest ordinary toast under pressure; never evict
    /// persistent errors automatically. `None` means all slots require dismissal.
    pub fn push(
        &mut self,
        notification: Notification,
        now: Duration,
        timestamp: Arc<str>,
    ) -> Option<u64> {
        self.tick(now);
        if let Some(old) = self.entries.iter().find(|t| {
            t.exiting.is_none()
                && t.notification.category == notification.category
                && t.notification.title == notification.title
                && t.notification.message == notification.message
        }) {
            return Some(old.id);
        }
        if self.entries.len() == LIMIT {
            let index = self
                .entries
                .iter()
                .rposition(|t| t.notification.duration.is_some())?;
            self.entries.remove(index);
        }
        self.serial += 1;
        let id = self.serial;
        self.entries.insert(
            0,
            Toast {
                id,
                timestamp,
                created: now,
                entered: now
                    + if self.reduced_motion {
                        Duration::ZERO
                    } else {
                        ENTER
                    },
                last: now,
                remaining: notification.duration,
                notification,
                hovered: false,
                exiting: None,
            },
        );
        Some(id)
    }
    pub fn tick(&mut self, now: Duration) {
        for toast in &mut self.entries {
            let start = toast.last.max(toast.entered);
            if toast.exiting.is_none()
                && !toast.hovered
                && now >= start
                && let Some(left) = &mut toast.remaining
            {
                let elapsed = now - start;
                if elapsed >= *left {
                    toast.exiting = Some(start + *left);
                    *left = Duration::ZERO;
                } else {
                    *left -= elapsed;
                }
            }
            toast.last = toast.last.max(now);
        }
        let exit = if self.reduced_motion {
            Duration::ZERO
        } else {
            EXIT
        };
        self.entries
            .retain(|t| t.exiting.is_none_or(|at| now.saturating_sub(at) < exit));
    }
    pub fn hover(&mut self, id: u64, hovered: bool, now: Duration) {
        self.tick(now);
        if let Some(t) = self.entries.iter_mut().find(|t| t.id == id) {
            t.hovered = hovered;
        }
    }
    pub fn dismiss(&mut self, id: u64, now: Duration) {
        if let Some(t) = self
            .entries
            .iter_mut()
            .find(|t| t.id == id && t.exiting.is_none())
        {
            t.exiting = Some(now);
        }
        self.tick(now);
    }
    pub fn animating(&self, now: Duration) -> bool {
        self.entries.iter().any(|t| {
            t.exiting.is_some()
                || !self.reduced_motion && now < t.entered
                || t.remaining.is_some() && !t.hovered
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info() -> Notification {
        Notification::new(Category::Information, "Info", "hello")
    }
    fn add(m: &mut Manager, n: Notification) -> u64 {
        m.push(n, Duration::ZERO, "12:00:00 UTC".into()).unwrap()
    }
    #[test]
    fn progress_expires_then_fades_and_stops() {
        let mut m = Manager::default();
        add(&mut m, info());
        assert_eq!(m.entries[0].frame(Duration::ZERO, false).progress, 1.);
        assert_eq!(m.entries[0].frame(Duration::ZERO, false).opacity, 0.);
        let mut previous = 1.;
        for millis in [180, 500, 1000, 2000, 4000, 4180] {
            let now = Duration::from_millis(millis);
            m.tick(now);
            let frame = m.entries[0].frame(now, false);
            assert!(frame.progress <= previous);
            previous = frame.progress;
        }
        assert_eq!(previous, 0.);
        m.tick(Duration::from_millis(4280));
        assert!(
            m.entries[0]
                .frame(Duration::from_millis(4280), false)
                .opacity
                < 1.
        );
        m.tick(Duration::from_millis(4400));
        assert!(m.entries.is_empty());
        assert!(!m.animating(Duration::from_secs(5)));
    }
    #[test]
    fn hover_preserves_remaining_lifetime_without_a_resume_jump() {
        let mut m = Manager::default();
        let id = add(&mut m, info());
        m.hover(id, true, Duration::from_secs(1));
        let before = m.entries[0].remaining;
        m.tick(Duration::from_secs(100));
        assert_eq!(m.entries[0].remaining, before);
        assert!(!m.animating(Duration::from_secs(100)));
        m.hover(id, false, Duration::from_secs(100));
        m.tick(Duration::from_millis(100100));
        assert_eq!(
            m.entries[0].remaining,
            before.map(|d| d - Duration::from_millis(100))
        );
    }
    #[test]
    fn bounded_newest_first_deduplicates_and_protects_persistent_items() {
        let mut m = Manager::default();
        let first = add(&mut m, info().persistent());
        assert_eq!(add(&mut m, info().persistent()), first);
        for i in 0..100 {
            add(
                &mut m,
                Notification::new(Category::Success, "Saved", &i.to_string()),
            );
        }
        assert_eq!(m.entries.len(), LIMIT);
        assert_eq!(m.entries[0].notification.message.as_ref(), "99");
        assert!(m.entries.iter().any(|t| t.id == first));
        m.dismiss(first, Duration::from_secs(1));
        m.tick(Duration::from_secs(2));
        assert!(!m.entries.iter().any(|t| t.id == first));
    }
    #[test]
    fn stalled_frames_reduced_motion_and_duration_overrides() {
        let mut m = Manager {
            reduced_motion: true,
            ..Default::default()
        };
        add(&mut m, info().for_duration(Duration::from_secs(1)));
        assert_eq!(m.entries[0].frame(Duration::ZERO, true).opacity, 1.);
        m.tick(Duration::from_secs(10));
        assert!(m.entries.is_empty());
        let id = add(&mut m, info().persistent());
        m.tick(Duration::from_secs(10));
        assert!(!m.animating(Duration::from_secs(10)));
        m.dismiss(id, Duration::from_secs(10));
        assert!(m.entries.is_empty());
    }
}
