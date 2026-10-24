//! A replaceable request slot, with cooperative cancellation and no input-thread joins.
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread;

type WakeFn = std::sync::Arc<dyn Fn() + Send + Sync>;
static WAKER: std::sync::Mutex<Option<WakeFn>> = std::sync::Mutex::new(None);
pub struct WakeGuard;
impl WakeGuard {
    pub fn install(wake: WakeFn) -> Self {
        *WAKER.lock().unwrap() = Some(wake);
        Self
    }
}
impl Drop for WakeGuard {
    fn drop(&mut self) {
        *WAKER.lock().unwrap() = None;
    }
}
pub fn wake() {
    if let Some(wake) = WAKER.lock().unwrap().as_ref() {
        wake();
    }
}

pub struct Cancel {
    generation: u64,
    current: Arc<AtomicU64>,
}
impl Cancel {
    pub fn cancelled(&self) -> bool {
        self.current.load(Ordering::Relaxed) != self.generation
    }
}
struct Slot<J> {
    job: Option<(u64, J)>,
    closed: bool,
}
pub struct LatestWorker<J, R> {
    slot: Arc<(Mutex<Slot<J>>, Condvar)>,
    current: Arc<AtomicU64>,
    results: mpsc::Receiver<(u64, R)>,
}
impl<J: Send + 'static, R: Send + 'static> LatestWorker<J, R> {
    pub fn new(mut run: impl FnMut(J, &Cancel) -> R + Send + 'static) -> Self {
        let slot = Arc::new((
            Mutex::new(Slot::<J> {
                job: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let current = Arc::new(AtomicU64::new(0));
        let (tx, results) = mpsc::channel();
        let worker_slot = Arc::clone(&slot);
        let worker_current = Arc::clone(&current);
        thread::spawn(move || {
            loop {
                let job = {
                    let (lock, ready) = &*worker_slot;
                    let mut slot = lock.lock().unwrap();
                    while slot.job.is_none() && !slot.closed {
                        slot = ready.wait(slot).unwrap();
                    }
                    if slot.closed {
                        break;
                    }
                    slot.job.take().unwrap()
                };
                let cancel = Cancel {
                    generation: job.0,
                    current: Arc::clone(&worker_current),
                };
                let result = run(job.1, &cancel);
                if !cancel.cancelled() && tx.send((job.0, result)).is_err() {
                    break;
                }
                wake();
            }
        });
        Self {
            slot,
            current,
            results,
        }
    }
    pub fn submit(&self, job: J) {
        let generation = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        let (lock, ready) = &*self.slot;
        lock.lock().unwrap().job = Some((generation, job));
        ready.notify_one();
    }
    pub fn cancel(&self) {
        self.current.fetch_add(1, Ordering::Relaxed);
        self.slot.0.lock().unwrap().job = None;
    }
    pub fn poll(&self) -> Option<R> {
        self.results
            .try_iter()
            .filter(|(id, _)| *id == self.current.load(Ordering::Relaxed))
            .last()
            .map(|(_, result)| result)
    }
}
impl<J, R> Drop for LatestWorker<J, R> {
    fn drop(&mut self) {
        self.current.fetch_add(1, Ordering::Relaxed);
        self.slot.0.lock().unwrap().closed = true;
        self.slot.1.notify_one();
    }
}

/// One shared preparation thread, latest work per independent document/request.
/// The per-key generation cancels running work without cancelling another server.
type KeyedShared<K, J, R> = Arc<(Mutex<KeyedSlot<K, J, R>>, Condvar)>;
pub struct KeyedWorker<K, J, R> {
    slot: KeyedShared<K, J, R>,
    generations: std::collections::BTreeMap<K, Arc<AtomicU64>>,
    serial: u64,
}
struct KeyedSlot<K, J, R> {
    results: std::collections::BTreeMap<K, (u64, R)>,
    ready: std::collections::VecDeque<K>,
    jobs: std::collections::BTreeMap<K, (u64, Arc<AtomicU64>, J)>,
    order: std::collections::VecDeque<K>,
    retired: Vec<Box<dyn Send>>,
    closed: bool,
}
impl<K: Ord + Clone + Send + 'static, J: Send + 'static, R: Send + 'static> KeyedWorker<K, J, R> {
    pub fn new(mut run: impl FnMut(J, &Cancel) -> R + Send + 'static) -> Self {
        let slot = Arc::new((
            Mutex::new(KeyedSlot::<K, J, R> {
                results: Default::default(),
                ready: Default::default(),
                jobs: Default::default(),
                order: Default::default(),
                retired: Vec::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker = slot.clone();
        thread::spawn(move || {
            loop {
                let (retired, job) = {
                    let mut slot = worker.0.lock().unwrap();
                    while slot.order.is_empty() && slot.retired.is_empty() && !slot.closed {
                        slot = worker.1.wait(slot).unwrap();
                    }
                    if slot.closed {
                        break;
                    }
                    let retired = std::mem::take(&mut slot.retired);
                    let job = slot.order.pop_front().map(|key| {
                        let job = slot.jobs.remove(&key).unwrap();
                        (key, job)
                    });
                    (retired, job)
                };
                drop(retired);
                let Some((key, (generation, current, job))) = job else {
                    continue;
                };
                let cancel = Cancel {
                    generation,
                    current,
                };
                let result = run(job, &cancel);
                if !cancel.cancelled() {
                    let old = {
                        let mut slot = worker.0.lock().unwrap();
                        if !slot.results.contains_key(&key) {
                            slot.ready.push_back(key.clone());
                        }
                        slot.results.insert(key, (generation, result))
                    };
                    drop(old); // Replacement destruction stays on the worker.
                    wake();
                }
            }
        });
        Self {
            slot,
            generations: Default::default(),
            serial: 0,
        }
    }
    pub fn submit(&mut self, key: K, job: J) {
        let current = self
            .generations
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        self.serial += 1;
        let generation = self.serial;
        current.store(generation, Ordering::Relaxed);
        let mut slot = self.slot.0.lock().unwrap();
        if !slot.jobs.contains_key(&key) {
            slot.order.push_back(key.clone());
        }
        if let Some((_, _, old)) = slot.jobs.insert(key, (generation, current, job)) {
            slot.retired.push(Box::new(old));
        }
        self.slot.1.notify_one();
    }
    pub fn poll(&mut self) -> Option<R> {
        // Each call is bounded even when obsolete results are queued.
        for _ in 0..8 {
            let (key, generation, result) = {
                let mut slot = self.slot.0.lock().unwrap();
                let key = slot.ready.pop_front()?;
                let (generation, result) = slot.results.remove(&key).unwrap();
                (key, generation, result)
            };
            if self
                .generations
                .get(&key)
                .is_some_and(|g| g.load(Ordering::Relaxed) == generation)
            {
                self.generations.remove(&key);
                return Some(result);
            }
            self.retire(result);
        }
        wake();
        None
    }
    pub fn retire<T: Send + 'static>(&self, value: T) {
        self.slot.0.lock().unwrap().retired.push(Box::new(value));
        self.slot.1.notify_one();
    }
    pub fn cancel_all(&mut self) {
        for generation in self.generations.values() {
            generation.fetch_add(1, Ordering::Relaxed);
        }
        self.generations.clear();
        let mut slot = self.slot.0.lock().unwrap();
        let jobs = std::mem::take(&mut slot.jobs);
        slot.retired.push(Box::new(jobs));
        let results = std::mem::take(&mut slot.results);
        slot.retired.push(Box::new(results));
        slot.ready.clear();
        slot.order.clear();
    }
}
impl<K, J, R> Drop for KeyedWorker<K, J, R> {
    fn drop(&mut self) {
        for generation in self.generations.values() {
            generation.fetch_add(1, Ordering::Relaxed);
        }
        self.slot.0.lock().unwrap().closed = true;
        self.slot.1.notify_one();
    }
}

/// Cooperative UI work budget. Exhaustion schedules another wake, so unread
/// results cannot be stranded after the original coalesced wake is consumed.
pub struct DrainBudget {
    started: std::time::Instant,
    remaining: usize,
}
impl DrainBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            started: std::time::Instant::now(),
            remaining: limit,
        }
    }
    pub fn allow(&mut self) -> bool {
        if self.remaining == 0 || self.started.elapsed() >= std::time::Duration::from_millis(2) {
            wake();
            return false;
        }
        self.remaining -= 1;
        true
    }
}

#[cfg(test)]
mod keyed_tests {
    use super::*;
    #[test]
    fn latest_per_key_is_fair_and_cancelled_generations_cannot_reappear() {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let mut worker = KeyedWorker::new(move |value: u64, cancel| {
            if value == 0 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            (value, cancel.cancelled())
        });
        worker.submit("a", 0);
        started
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.submit("a", 1);
        worker.submit("a", 2);
        worker.submit("b", 3);
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut values = Vec::new();
        while values.len() < 2 && std::time::Instant::now() < deadline {
            if let Some(value) = worker.poll() {
                values.push(value);
            }
            thread::yield_now();
        }
        assert_eq!(values, [(2, false), (3, false)]);
        worker.submit("a", 4);
        worker.cancel_all();
        worker.submit("a", 5);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(value) = worker.poll() {
                assert_eq!(value, (5, false));
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
    }
}
