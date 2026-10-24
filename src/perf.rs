//! Optional monotonic per-event measurements. The default build has no recorder,
//! timestamp reads or locks. Enable with `--features perf`.
#[cfg(feature = "perf")]
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[cfg(feature = "perf")]
#[derive(Default)]
struct Recorder {
    samples: BTreeMap<&'static str, Vec<u64>>,
    waiting: Vec<Instant>,
    trace: Vec<(&'static str, String, Instant)>,
    depth: usize,
    max_depth: usize,
}
#[cfg(feature = "perf")]
static RECORDER: OnceLock<Mutex<Recorder>> = OnceLock::new();
#[cfg(feature = "perf")]
fn recorder() -> &'static Mutex<Recorder> {
    RECORDER.get_or_init(Default::default)
}

pub struct Timer {
    #[cfg(feature = "perf")]
    at: Instant,
}
impl Timer {
    #[inline]
    pub fn start() -> Self {
        Self {
            #[cfg(feature = "perf")]
            at: Instant::now(),
        }
    }
    #[inline]
    pub fn finish(self, stage: &'static str) {
        #[cfg(feature = "perf")]
        record(stage, self.at.elapsed());
        #[cfg(not(feature = "perf"))]
        let _ = stage;
    }
}
#[cfg(feature = "perf")]
pub fn record(stage: &'static str, duration: Duration) {
    let mut r = recorder().lock().unwrap();
    let samples = r.samples.entry(stage).or_default();
    if samples.len() < 100_000 {
        samples.push(duration.as_nanos().min(u64::MAX as u128) as u64);
    }
}
#[cfg(feature = "perf")]
pub fn trace(stage: &'static str, identity: String) {
    let mut r = recorder().lock().unwrap();
    if r.trace.len() < 100_000 {
        r.trace.push((stage, identity, Instant::now()));
    }
}
#[cfg(feature = "perf")]
pub fn enqueued() -> Instant {
    let at = Instant::now();
    let mut r = recorder().lock().unwrap();
    r.depth += 1;
    r.max_depth = r.max_depth.max(r.depth);
    at
}
#[cfg(feature = "perf")]
pub fn dispatched(at: Instant) {
    record("input_queue", at.elapsed());
    let mut r = recorder().lock().unwrap();
    r.depth = r.depth.saturating_sub(1);
}
#[cfg(feature = "perf")]
pub fn completed(at: Instant) {
    record("input_to_action", at.elapsed());
    recorder().lock().unwrap().waiting.push(at);
}
#[cfg(feature = "perf")]
pub fn frame_completed() {
    let waiting = std::mem::take(&mut recorder().lock().unwrap().waiting);
    for at in waiting {
        record("input_to_frame", at.elapsed());
    }
}
#[cfg(feature = "perf")]
pub fn reset() {
    *recorder().lock().unwrap() = Recorder::default();
}
#[cfg(feature = "perf")]
pub fn report() -> String {
    let r = recorder().lock().unwrap();
    let mut text = format!(
        "max_input_queue={} trace_events={}\n",
        r.max_depth,
        r.trace.len()
    );
    for (name, samples) in &r.samples {
        if samples.is_empty() {
            continue;
        }
        let mut samples = samples.clone();
        samples.sort_unstable();
        let p =
            |percent: usize| samples[(samples.len() * percent).div_ceil(100) - 1] as f64 / 1000.;
        text.push_str(&format!(
            "{name}: n={} p50_us={:.2} p95_us={:.2} p99_us={:.2} max_us={:.2}\n",
            samples.len(),
            p(50),
            p(95),
            p(99),
            p(100)
        ));
    }
    text
}
#[cfg(feature = "perf")]
pub fn samples(stage: &'static str) -> Vec<u64> {
    recorder()
        .lock()
        .unwrap()
        .samples
        .get(stage)
        .cloned()
        .unwrap_or_default()
}
#[cfg(feature = "perf")]
pub fn traces() -> Vec<(&'static str, String, Instant)> {
    recorder().lock().unwrap().trace.clone()
}
