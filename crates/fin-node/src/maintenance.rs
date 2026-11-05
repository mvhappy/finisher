//! The per-engine maintenance thread and the events it forwards to JS.
//!
//! Every [`TICK_EVERY`] the thread calls `Store::tick_with` under the store mutex; the store's
//! own cadence decides when a tick polls the folder (local 2 s, network 3 s, sync 10 s), and
//! only then imports and re-checks or re-acquires the lease. Between polls a tick only flushes
//! a due export and reports a status change. Without these ticks a follower never follows, a
//! lost lease is never re-acquired and nothing is exported before `close`.
//!
//! The ticks leave the heavy steps out (`TickOptions` off): a daily backup or compaction the
//! tick finds due is queued on the worker as a long call (`backup (auto)`, `compaction
//! (auto)`), so the mutex is not held through `VACUUM INTO` or compaction's folder work. The
//! import itself still runs under the mutex (a long follower import delays short calls; the
//! lock-only-around-DB-transactions rework is left for later).
//!
//! Every `status` event also refreshes the engine's cached status (what `status()` answers
//! during a long call).
//!
//! A tick is skipped while a long call has the store (`Busy`). A panic inside a tick drops the
//! store (`Panicked`, like any call) and ends the thread after a fatal `error` event. A tick
//! that fails with a database error emits a non-fatal `error` event and the thread goes on.
//!
//! # Events (JSON strings)
//!
//! - `{"type":"applied","tx":CommittedTx}`: a record imported from the folder;
//! - `{"type":"status","status":ProjectStatus}`: the status changed (lease ages ignored);
//! - `{"type":"rewound","branch","fromRev","quarantined","own","generation","incarnation"}`:
//!   an import took records out of `branch` from `fromRev` on (before the `applied` events
//!   that replace them); `{incarnation, n: generation}` is the branch's generation now;
//! - `{"type":"rebuilt","incarnation"}`: the database was rebuilt (at open or by `rebuild`);
//!   every rev read before may hold something else now (before the `applied` events after it);
//! - `{"type":"error","error":CoreError,"fatal":bool}`: a tick or an automatic backup or
//!   compaction failed; `fatal` when it panicked and the engine is dead;
//! - `{"type":"lagged","dropped":n}`: first after registration, when the buffer overflowed.
//!
//! `applied` covers every import pass: the ticks' and those run at open, by a takeover or a
//! rebuild (delivered by the next tick).
//!
//! They reach the `onEvent` callback through a weak (unref'd) threadsafe function, so a
//! registered callback does not keep the process alive. Events emitted before the first
//! `onEvent` are buffered, the newest [`BUFFERED`] kept.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use fin_store::{EngineEvent, StoreEvent, TickOptions};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use serde_json::json;

use crate::engine::{Shared, State, lock};
use crate::panic_error;
use crate::worker::Worker;

/// Ticks leave the daily backup and compaction to long calls (`worker::auto`).
const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

/// How often the maintenance thread ticks.
pub(crate) const TICK_EVERY: Duration = Duration::from_millis(500);

/// Events kept while no callback is registered.
pub(crate) const BUFFERED: usize = 1024;

/// The `onEvent` callback: weak (does not keep the event loop alive), errors not passed in.
pub(crate) type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;

#[derive(Default)]
struct Sink {
    callback: Option<EventFn>,
    buffered: VecDeque<String>,
    /// Events dropped from a full buffer (reported as `lagged`).
    dropped: u64,
}

/// Where events go: the registered callback, or a bounded buffer until there is one.
#[derive(Default)]
pub(crate) struct EventSink {
    sink: Mutex<Sink>,
}

impl EventSink {
    pub(crate) fn emit(&self, json: String) {
        let mut s = lock(&self.sink);
        match &s.callback {
            // Non-blocking: queued for the JS thread (the queue is unbounded).
            Some(cb) => {
                cb.call(json, ThreadsafeFunctionCallMode::NonBlocking);
            }
            None => {
                if s.buffered.len() == BUFFERED {
                    s.buffered.pop_front();
                    s.dropped += 1;
                }
                s.buffered.push_back(json);
            }
        }
    }

    /// Registers `callback` (replacing an earlier one) and hands it the buffered events.
    pub(crate) fn set(&self, callback: EventFn) {
        let mut s = lock(&self.sink);
        if s.dropped > 0 {
            let lagged = to_json(&EngineEvent::Lagged {
                dropped: s.dropped as u64,
            });
            callback.call(lagged, ThreadsafeFunctionCallMode::NonBlocking);
            s.dropped = 0;
        }
        for json in std::mem::take(&mut s.buffered) {
            callback.call(json, ThreadsafeFunctionCallMode::NonBlocking);
        }
        s.callback = Some(callback);
    }

    /// Drops the callback and the buffer (at close).
    pub(crate) fn clear(&self) {
        let mut s = lock(&self.sink);
        s.callback = None;
        s.buffered.clear();
        s.dropped = 0;
    }
}

struct Stop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

/// A running maintenance thread.
pub(crate) struct Maintenance {
    stop: Arc<Stop>,
    thread: JoinHandle<()>,
}

impl Maintenance {
    pub(crate) fn start(inner: &Shared, worker: Worker) -> Result<Maintenance> {
        let stop = Arc::new(Stop {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        });
        let (inner, signal) = (inner.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("fin-node-maintenance".to_string())
            .spawn(move || run(&inner, &signal, &worker))
            .map_err(|e| {
                crate::reject(CoreError::new(
                    "INTERNAL",
                    format!("cannot start the maintenance thread: {e}"),
                ))
            })?;
        Ok(Maintenance { stop, thread })
    }

    /// Stops the thread and waits for it (a running tick finishes first).
    pub(crate) fn stop(self) {
        self.signal_ref();
        let _ = self.thread.join();
    }

    /// Tells the thread to stop, without waiting for it.
    pub(crate) fn signal(self) {
        self.signal_ref();
    }

    fn signal_ref(&self) {
        *lock(&self.stop.stopped) = true;
        self.stop.wake.notify_all();
    }

    /// Whether the thread is still running (it ends early after a panic or close).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn running(&self) -> bool {
        !self.thread.is_finished()
    }
}

fn run(inner: &Shared, stop: &Stop, worker: &Worker) {
    loop {
        let deadline = Instant::now() + TICK_EVERY;
        let mut stopped = lock(&stop.stopped);
        while !*stopped {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            stopped = stop
                .wake
                .wait_timeout(stopped, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        if *stopped {
            return;
        }
        drop(stopped);
        if !tick(inner) {
            return;
        }
        crate::worker::auto(inner, worker);
    }
}

/// One tick; `false` once the engine is closed or dead.
fn tick(inner: &Shared) -> bool {
    let mut guard = lock(&inner.state);
    let store = match &mut *guard {
        State::Open(store) => store,
        State::Busy { .. } => return true,
        State::Closed | State::Panicked => return false,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(feature = "test-hooks")]
        if inner.hooks.take_tick_panic() {
            panic!("fin-node test panic in tick");
        }
        let events = store.tick_with(Instant::now(), LIGHT)?.events;
        let incarnation = events
            .iter()
            .any(|e| matches!(e, StoreEvent::Rewound { .. }))
            .then(|| store.incarnation());
        Ok::<_, fin_store::StoreError>((events, incarnation))
    }));
    match result {
        Ok(Ok((events, incarnation))) => {
            // Queued before the store is released: a commit that runs right after this tick
            // answers after these events reach JS (a rewind is seen before the new history's
            // first commit).
            for e in &events {
                if let StoreEvent::Status(st) = e
                    && let Ok(v) = serde_json::to_value(st)
                {
                    *lock(&inner.status) = Some(v);
                }
                inner.events.emit(event_json(e, incarnation.as_deref()));
            }
            drop(guard);
            true
        }
        Ok(Err(e)) => {
            drop(guard);
            let e: CoreError = e.into();
            inner.events.emit(error_json(&json!(e), false));
            true
        }
        Err(payload) => {
            let store = std::mem::replace(&mut *guard, State::Panicked);
            drop(guard);
            let _ = catch_unwind(AssertUnwindSafe(move || drop(store)));
            let e = panic_error(payload.as_ref());
            inner.events.emit(error_json(&json!(e), true));
            false
        }
    }
}

/// `e` as its [`EngineEvent`] JSON (the generated `EngineEvent` type of
/// `@finisher/core-types`).
pub(crate) fn event_json(e: &StoreEvent, incarnation: Option<&str>) -> String {
    to_json(&EngineEvent::from_store(e, incarnation))
}

/// An `error` event; `e` is the `CoreError` JSON.
pub(crate) fn error_json(e: &serde_json::Value, fatal: bool) -> String {
    to_json(&EngineEvent::Error {
        error: e.clone(),
        fatal,
    })
}

fn to_json(e: &EngineEvent) -> String {
    serde_json::to_string(e).expect("an engine event always serialises")
}
