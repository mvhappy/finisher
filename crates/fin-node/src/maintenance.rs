//! The per-engine maintenance thread and the events it forwards to JS.
//!
//! Every [`TICK_EVERY`] the thread calls `Store::tick` under the store mutex; the store's own
//! cadence decides when a tick polls the folder (local 2 s, network 3 s, sync 10 s), and only
//! then imports, re-checks or re-acquires the lease, takes the daily backup and compacts.
//! Between polls a tick only flushes a due export and reports a status change. Without these
//! ticks a follower never follows, a lost lease is never re-acquired and nothing is exported
//! before `close`.
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
//! - `{"type":"error","error":CoreError,"fatal":bool}`: a tick failed; `fatal` when it
//!   panicked and the engine is dead.
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
use fin_store::StoreEvent;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use serde_json::json;

use crate::engine::{Shared, State, lock};
use crate::panic_error;

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
                }
                s.buffered.push_back(json);
            }
        }
    }

    /// Registers `callback` (replacing an earlier one) and hands it the buffered events.
    pub(crate) fn set(&self, callback: EventFn) {
        let mut s = lock(&self.sink);
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
    pub(crate) fn start(inner: &Shared) -> Result<Maintenance> {
        let stop = Arc::new(Stop {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        });
        let (inner, signal) = (inner.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("fin-node-maintenance".to_string())
            .spawn(move || run(&inner, &signal))
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
        *lock(&self.stop.stopped) = true;
        self.stop.wake.notify_all();
        let _ = self.thread.join();
    }

    /// Whether the thread is still running (it ends early after a panic or close).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn running(&self) -> bool {
        !self.thread.is_finished()
    }
}

fn run(inner: &Shared, stop: &Stop) {
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
        let events = store.tick(Instant::now())?;
        let incarnation = events
            .iter()
            .any(|e| matches!(e, StoreEvent::Rewound { .. }))
            .then(|| store.incarnation());
        Ok::<_, fin_store::StoreError>((events, incarnation))
    }));
    match result {
        Ok(Ok((events, incarnation))) => {
            drop(guard);
            for e in &events {
                inner.events.emit(event_json(e, incarnation.as_deref()));
            }
            true
        }
        Ok(Err(e)) => {
            drop(guard);
            inner.events.emit(error_json(&e.into(), false));
            true
        }
        Err(payload) => {
            let store = std::mem::replace(&mut *guard, State::Panicked);
            drop(guard);
            let _ = catch_unwind(AssertUnwindSafe(move || drop(store)));
            inner
                .events
                .emit(error_json(&panic_error(payload.as_ref()), true));
            false
        }
    }
}

fn event_json(e: &StoreEvent, incarnation: Option<&str>) -> String {
    let v = match e {
        StoreEvent::Applied(tx) => json!({ "type": "applied", "tx": tx }),
        StoreEvent::Status(status) => json!({ "type": "status", "status": status }),
        StoreEvent::Rewound {
            branch,
            from_rev,
            quarantined,
            own,
            generation,
        } => json!({
            "type": "rewound",
            "branch": branch,
            "fromRev": from_rev,
            "quarantined": quarantined,
            "own": own,
            "generation": generation,
            "incarnation": incarnation.unwrap_or_default(),
        }),
    };
    v.to_string()
}

fn error_json(e: &CoreError, fatal: bool) -> String {
    json!({ "type": "error", "error": e, "fatal": fatal }).to_string()
}
