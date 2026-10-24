//! Nonblocking producers with queue-age/size observation. Size estimates describe
//! payload volume, not retained heap bytes (Rope and Arc storage can be shared).
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub depth: usize,
    pub approximate_bytes: usize,
    pub oldest_age: Duration,
    pub peak_depth: usize,
    pub peak_approximate_bytes: usize,
    pub worst_age: Duration,
}
#[derive(Default)]
struct State {
    serial: u64,
    pending: BTreeMap<u64, (Instant, usize)>,
    bytes: usize,
    peak: usize,
    peak_bytes: usize,
    worst_age: Duration,
}
#[derive(Clone, Default)]
pub struct Metrics(Arc<Mutex<State>>);
impl Metrics {
    pub fn snapshot(&self) -> Snapshot {
        let state = self.0.lock().unwrap();
        Snapshot {
            depth: state.pending.len(),
            approximate_bytes: state.bytes,
            oldest_age: state
                .pending
                .first_key_value()
                .map_or(Duration::ZERO, |(_, (at, _))| at.elapsed()),
            peak_depth: state.peak,
            peak_approximate_bytes: state.peak_bytes,
            worst_age: state.worst_age,
        }
    }
    fn remove(&self, id: u64) {
        let mut state = self.0.lock().unwrap();
        if let Some((at, bytes)) = state.pending.remove(&id) {
            state.bytes -= bytes;
            state.worst_age = state.worst_age.max(at.elapsed());
        }
    }
}
pub struct Sender<T> {
    tx: mpsc::Sender<(u64, T)>,
    metrics: Metrics,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            metrics: self.metrics.clone(),
        }
    }
}
pub struct Receiver<T> {
    rx: mpsc::Receiver<(u64, T)>,
    metrics: Metrics,
}
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    let metrics = Metrics::default();
    (
        Sender {
            tx,
            metrics: metrics.clone(),
        },
        Receiver { rx, metrics },
    )
}
impl<T> Sender<T> {
    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        self.send_sized(value, 0)
    }
    pub fn send_sized(&self, value: T, bytes: usize) -> Result<(), mpsc::SendError<T>> {
        let id = {
            let mut state = self.metrics.0.lock().unwrap();
            state.serial += 1;
            let id = state.serial;
            state.pending.insert(id, (Instant::now(), bytes));
            state.bytes += bytes;
            state.peak_bytes = state.peak_bytes.max(state.bytes);
            state.peak = state.peak.max(state.pending.len());
            id
        };
        self.tx.send((id, value)).map_err(|e| {
            self.metrics.remove(id);
            mpsc::SendError(e.0.1)
        })
    }
}
impl<T> Receiver<T> {
    fn take(&self, (id, value): (u64, T)) -> T {
        self.metrics.remove(id);
        value
    }
    pub fn recv_timeout(&self, wait: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(wait).map(|v| self.take(v))
    }
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.rx.try_recv().map(|v| self.take(v))
    }
    pub fn try_iter(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.metrics.0.lock().unwrap();
        state.pending.clear();
        state.bytes = 0;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_volume_age_and_disconnect_are_accounted() {
        let (tx, rx) = channel();
        let metrics = tx.metrics();
        tx.send_sized(1, 20).unwrap();
        tx.send_sized(2, 30).unwrap();
        assert_eq!(metrics.snapshot().depth, 2);
        assert_eq!(metrics.snapshot().approximate_bytes, 50);
        assert_eq!(rx.try_recv().unwrap(), 1);
        assert_eq!(metrics.snapshot().approximate_bytes, 30);
        assert_eq!(rx.try_recv().unwrap(), 2);
        assert_eq!(metrics.snapshot().oldest_age, Duration::ZERO);
        drop(rx);
        assert!(tx.send_sized(3, 100).is_err());
        assert_eq!(metrics.snapshot().depth, 0);
    }
}
