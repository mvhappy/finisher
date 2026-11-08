//! How host events reach JS (5C-1 plan, Decision 11a).
//!
//! - **One bounded queue.** Events are JSON strings handed to one unref'd threadsafe function.
//!   At most `eventQueue` (1,024) events, of at most `eventQueueBytes` (16 MiB) of JSON
//!   together, wait for the JS thread; a reader thread with one more to hand over waits until
//!   JS takes some (one event larger than the byte bound still goes when the queue is empty).
//!   The readers then stop reading, the pipes fill and the worker blocks on its own writes:
//!   backpressure reaches the worker, memory does not grow. The host counts that wait as
//!   busy, not silent, so a slow event loop never looks hung. The waiting happens here,
//!   outside napi (a non-blocking call into an unbounded queue after our own count allows
//!   it): napi's blocking mode would block inside napi-rs's abort lock, which the JS thread
//!   takes at teardown.
//! - **Lifecycle events are not counted.** `spawned`, `exit`, `groupEmpty` and the spawn
//!   answers never wait: a process has one of each (an `exit` is ≤ 8 KiB of stderr tail), and
//!   the spawn thread, which sends most of them, must not stall behind a slow JS thread.
//!   They still go through the same queue, so the order is kept. (The spawn thread can still
//!   wait for a process's own events lock, held by its reader while that reader waits here:
//!   only for an `exit` the spawn thread reports itself, an abandoned or unwaited process.)
//! - **Progress is coalesced.** Per process the newest `progress` is kept and handed over at
//!   most every 250 ms; a trailing one is handed over by the flusher thread. Any other event
//!   of the process first hands over the pending one, so the order of each process's events is
//!   kept and the last progress comes before `result`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use fin_jobhost::{HostEvent, ProcId, WorkerMsg};
use napi::Status;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

use super::napi_types::event_json;

/// What crosses to the JS thread: the JSON and, if it counts against the bound, the bytes it
/// was counted with.
pub(crate) struct Handed {
    pub json: String,
    weight: Option<usize>,
}

/// The JS `onEvent` callback: weak (does not keep the event loop alive), no error argument.
pub(crate) type EventFn = ThreadsafeFunction<Handed, (), String, Status, false, true>;

/// The shortest gap between two progress events of one process.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How often the flusher looks for trailing progress while some is pending.
const FLUSH_TICK: Duration = Duration::from_millis(25);
/// How long a waiting reader sleeps before it checks for a close.
const WAIT_TICK: Duration = Duration::from_millis(100);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The counted events handed over and not yet taken by the JS thread.
#[derive(Default)]
struct Load {
    n: usize,
    bytes: usize,
}

impl Load {
    /// Whether one more of `len` bytes fits: under both bounds, or alone in the queue.
    fn admits(&self, len: usize, cap: usize, cap_bytes: usize) -> bool {
        self.n < cap && (self.n == 0 || self.bytes + len <= cap_bytes)
    }
}

pub(crate) struct Inflight {
    load: Mutex<Load>,
    taken: Condvar,
}

impl Inflight {
    pub(crate) fn new() -> Arc<Inflight> {
        Arc::new(Inflight {
            load: Mutex::default(),
            taken: Condvar::new(),
        })
    }

    /// The JS thread took `h` (runs there, just before the callback).
    pub(crate) fn take(&self, h: &Handed) {
        self.release(h.weight);
    }

    fn release(&self, weight: Option<usize>) {
        let Some(len) = weight else { return };
        let mut l = lock(&self.load);
        l.n = l.n.saturating_sub(1);
        l.bytes = l.bytes.saturating_sub(len);
        self.taken.notify_all();
    }
}

/// The bounded queue to JS.
struct Delivery {
    tsfn: EventFn,
    cap: usize,
    cap_bytes: usize,
    inflight: Arc<Inflight>,
    closed: AtomicBool,
    #[cfg(feature = "test-hooks")]
    paused_until: Mutex<Option<Instant>>,
}

impl Delivery {
    /// Hands `json` over; `counted`, it first waits while the bound is reached. Dropped once
    /// closed.
    fn deliver(&self, json: String, counted: bool) {
        #[cfg(feature = "test-hooks")]
        self.wait_pause();
        if counted {
            let mut l = lock(&self.inflight.load);
            while !l.admits(json.len(), self.cap, self.cap_bytes) {
                if self.closed.load(Ordering::Acquire) {
                    return;
                }
                l = self
                    .inflight
                    .taken
                    .wait_timeout(l, WAIT_TICK)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            l.n += 1;
            l.bytes += json.len();
        }
        let weight = counted.then_some(json.len());
        if self.closed.load(Ordering::Acquire) {
            return self.inflight.release(weight);
        }
        #[cfg(feature = "test-hooks")]
        let json = stamp(json);
        let h = Handed { json, weight };
        if self.tsfn.call(h, ThreadsafeFunctionCallMode::NonBlocking) != Status::Ok {
            // Closing (the env is going away): nothing will take it.
            self.inflight.release(weight);
        }
    }

    /// `pauseEventsForTest`: a slow JS thread, seen from the readers.
    #[cfg(feature = "test-hooks")]
    fn wait_pause(&self) {
        loop {
            let until = *lock(&self.paused_until);
            let Some(until) = until else { return };
            let now = Instant::now();
            if now >= until || self.closed.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep((until - now).min(WAIT_TICK));
        }
    }
}

/// Test builds: `handedAtMsForTest` (wall-clock ms) is added to every event as it is handed
/// over, so tests can time the hand-over rather than the JS thread's receipt.
#[cfg(feature = "test-hooks")]
fn stamp(json: String) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    match json.strip_prefix('{') {
        Some(rest) if rest != "}" => format!("{{\"handedAtMsForTest\":{ms},{rest}"),
        _ => json,
    }
}

/// One process's coalescing state.
#[derive(Default)]
struct Slot {
    /// The newest progress not yet handed over.
    pending: Option<HostEvent>,
    /// When the last progress was handed over.
    last: Option<Instant>,
}

/// The flusher's wake-up.
#[derive(Default)]
struct Flush {
    stop: bool,
    pending: bool,
}

/// The host's [`fin_jobhost::EventSink`] target.
pub(crate) struct Events {
    delivery: Delivery,
    slots: Mutex<HashMap<ProcId, Arc<Mutex<Slot>>>>,
    flush: Mutex<Flush>,
    wake: Condvar,
}

impl Events {
    /// Starts the flusher thread; [`Events::close`] stops it.
    pub(crate) fn start(
        tsfn: EventFn,
        inflight: Arc<Inflight>,
        cap: usize,
        cap_bytes: usize,
    ) -> napi::Result<Arc<Events>> {
        let events = Arc::new(Events {
            delivery: Delivery {
                tsfn,
                cap: cap.max(1),
                cap_bytes: cap_bytes.max(1),
                inflight,
                closed: AtomicBool::new(false),
                #[cfg(feature = "test-hooks")]
                paused_until: Mutex::new(None),
            },
            slots: Mutex::default(),
            flush: Mutex::default(),
            wake: Condvar::new(),
        });
        let e = events.clone();
        std::thread::Builder::new()
            .name("jobhost-events".into())
            .spawn(move || e.flusher())
            .map_err(|e| napi::Error::from_reason(format!("cannot start the event thread: {e}")))?;
        Ok(events)
    }

    /// Hands `json` over without waiting (a spawn's answer).
    pub(crate) fn deliver_now(&self, json: String) {
        self.delivery.deliver(json, false);
    }

    /// Called by the host's threads (readers, waiters, the spawn thread).
    pub(crate) fn on_event(&self, e: HostEvent) {
        let proc = match &e {
            HostEvent::GroupEmpty { .. }
            | HostEvent::Spawned { .. }
            | HostEvent::AbandonedExit { .. } => {
                return self.delivery.deliver(event_json(e), false);
            }
            HostEvent::Hello { proc, .. }
            | HostEvent::Message { proc, .. }
            | HostEvent::Stderr { proc, .. }
            | HostEvent::Exit { proc, .. } => *proc,
        };
        let progress = matches!(
            &e,
            HostEvent::Message {
                msg: WorkerMsg::Progress(_),
                ..
            }
        );
        let exit = matches!(e, HostEvent::Exit { .. });
        let slot = self.slot(proc);
        let mut s = lock(&slot);
        if progress {
            let now = Instant::now();
            if s.last.is_none_or(|t| now >= t + PROGRESS_EVERY) {
                // The newest wins: a pending older value is dropped.
                s.pending = None;
                s.last = Some(now);
                self.delivery.deliver(event_json(e), true);
            } else {
                s.pending = Some(e);
                drop(s);
                self.wake_flusher();
            }
            return;
        }
        if let Some(p) = s.pending.take() {
            s.last = Some(Instant::now());
            self.delivery.deliver(event_json(p), true);
        }
        self.delivery.deliver(event_json(e), !exit);
        drop(s);
        if exit {
            lock(&self.slots).remove(&proc);
        }
    }

    fn slot(&self, proc: ProcId) -> Arc<Mutex<Slot>> {
        lock(&self.slots).entry(proc).or_default().clone()
    }

    fn wake_flusher(&self) {
        lock(&self.flush).pending = true;
        self.wake.notify_one();
    }

    /// Hands over trailing progress once its 250 ms are up. A slot whose reader is handing
    /// something over (locked) is retried at the next tick.
    fn flusher(&self) {
        loop {
            {
                let mut f = lock(&self.flush);
                while !f.stop && !f.pending {
                    f = self.wake.wait(f).unwrap_or_else(|e| e.into_inner());
                }
                if f.stop {
                    return;
                }
                f.pending = false;
            }
            std::thread::sleep(FLUSH_TICK);
            let slots: Vec<_> = lock(&self.slots).values().cloned().collect();
            let mut left = false;
            for slot in slots {
                let mut s = match slot.try_lock() {
                    Ok(s) => s,
                    Err(TryLockError::Poisoned(e)) => e.into_inner(),
                    Err(TryLockError::WouldBlock) => {
                        left = true;
                        continue;
                    }
                };
                if s.pending.is_none() {
                    continue;
                }
                let now = Instant::now();
                if s.last.is_some_and(|t| now < t + PROGRESS_EVERY) {
                    left = true;
                    continue;
                }
                if let Some(p) = s.pending.take() {
                    s.last = Some(now);
                    self.delivery.deliver(event_json(p), true);
                }
            }
            if left {
                lock(&self.flush).pending = true;
            }
        }
    }

    /// Stops the flusher and drops whatever is handed over from now on.
    pub(crate) fn close(&self) {
        self.delivery.closed.store(true, Ordering::Release);
        self.delivery.inflight.taken.notify_all();
        lock(&self.flush).stop = true;
        self.wake.notify_one();
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn pause_for_test(&self, ms: u32) {
        *lock(&self.delivery.paused_until) =
            Some(Instant::now() + Duration::from_millis(u64::from(ms)));
    }
}
