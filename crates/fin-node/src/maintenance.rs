//! The per-engine maintenance thread and the events it forwards to JS.
//!
//! Every [`TICK_EVERY`] the thread runs `Store::tick_phased`; the store's own cadence decides
//! when a tick polls the folder (local 2 s, network 1 s, sync 10 s), and only then imports and
//! re-checks or re-acquires the lease. Between polls a tick only flushes a due export and
//! reports a status change. Without these ticks a follower never follows, a lost lease is
//! never re-acquired and nothing is exported before `close`.
//!
//! The store mutex is taken step by step, around the tick's database work, never across its
//! folder I/O: an import's scan and verification, an export's writes and read-backs and the
//! lease re-check run without it, so short calls answer meanwhile (`fin_store::tick` module
//! docs, "Locking"). A long call may take the store between two steps: the tick then ends
//! there, and the next starts afresh. Acquiring the lease still runs under the mutex.
//!
//! The ticks leave the heavy steps out (`TickOptions` off): after each tick, the daily backup
//! or compaction it found due runs as automatic work (`crate::auto`): only once the engine is
//! quiet, the backup off the store on a thread of its own, compaction one step at a time
//! (under the mutex).
//!
//! Every `status` event also refreshes the engine's cached status (what `status()` answers
//! during a long call).
//!
//! A tick is skipped while a long call has the store (`Busy`). A panic inside a tick step drops
//! the store (`Panicked`, like any call) and ends the thread after a fatal `error` event. A tick
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
//! - `{"type":"error","error":CoreError,"fatal":bool}`: a tick or an automatic compaction
//!   failed; `fatal` when it panicked and the engine is dead;
//! - `{"type":"lagged","dropped":n}`: first after registration, when the buffer overflowed;
//! - `{"type":"collect","collect":"default"|"never"}`: the project file's collection flag
//!   changed (this server set it, or another did and a re-read found it); passed to the
//!   server's journal before it is forwarded.
//!
//! `applied` covers every import pass: the ticks' and those run at open, by a takeover or a
//! rebuild (delivered by the next tick).
//!
//! They reach the `onEvent` callback through a weak (unref'd) threadsafe function, so a
//! registered callback does not keep the process alive. Events emitted before the first
//! `onEvent` are buffered, the newest [`BUFFERED`] kept.
//!
//! Each event is numbered as it is emitted (`seq`, from 1; `lagged` is 0), the callback's
//! second argument. `statusAt()` answers with the last number its status covers (review #39):
//! the events up to it were emitted before that status was read, so a `project.events`
//! subscriber starting from it drops them instead of replaying older state after it.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use fin_store::{EngineEvent, Store, StoreAccess, StoreEvent, TickOptions};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use serde_json::json;

use crate::engine::{Shared, State, lock};
use crate::panic_error;

/// Ticks leave the daily backup and compaction to `crate::auto`.
const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

/// How often the maintenance thread ticks.
pub(crate) const TICK_EVERY: Duration = Duration::from_millis(500);

/// Events kept while no callback is registered.
pub(crate) const BUFFERED: usize = 1024;

/// The `onEvent` callback: weak (does not keep the event loop alive), errors not passed in.
pub(crate) type EventFn = ThreadsafeFunction<EventArgs, (), EventArgs, Status, false, true>;

/// What the callback is called with: the event's JSON and its `seq`.
pub(crate) type EventArgs = FnArgs<(String, f64)>;

#[derive(Default)]
struct Sink {
    callback: Option<EventFn>,
    buffered: VecDeque<(String, u64)>,
    /// Events dropped from a full buffer (reported as `lagged`).
    dropped: u64,
    /// The last event's number.
    seq: u64,
}

fn args(json: String, seq: u64) -> EventArgs {
    // Exact as an f64 up to 2^53 events.
    (json, seq as f64).into()
}

/// Where events go: the registered callback, or a bounded buffer until there is one.
#[derive(Default)]
pub(crate) struct EventSink {
    sink: Mutex<Sink>,
}

impl EventSink {
    pub(crate) fn emit(&self, json: String) {
        let mut s = lock(&self.sink);
        s.seq += 1;
        let seq = s.seq;
        match &s.callback {
            // Non-blocking: queued for the JS thread (the queue is unbounded).
            Some(cb) => {
                cb.call(args(json, seq), ThreadsafeFunctionCallMode::NonBlocking);
            }
            None => {
                if s.buffered.len() == BUFFERED {
                    s.buffered.pop_front();
                    s.dropped += 1;
                }
                s.buffered.push_back((json, seq));
            }
        }
    }

    /// The last event's number: every event up to it has been emitted.
    pub(crate) fn seq(&self) -> u64 {
        lock(&self.sink).seq
    }

    /// Registers `callback` (replacing an earlier one) and hands it the buffered events.
    pub(crate) fn set(&self, callback: EventFn) {
        let mut s = lock(&self.sink);
        if s.dropped > 0 {
            let lagged = to_json(&EngineEvent::Lagged { dropped: s.dropped });
            callback.call(args(lagged, 0), ThreadsafeFunctionCallMode::NonBlocking);
            s.dropped = 0;
        }
        for (json, seq) in std::mem::take(&mut s.buffered) {
            callback.call(args(json, seq), ThreadsafeFunctionCallMode::NonBlocking);
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
        if !tick(inner) || !crate::auto::after_tick(inner) {
            return;
        }
    }
}

/// One tick; `false` once the engine is closed or dead.
fn tick(inner: &Shared) -> bool {
    match &*lock(&inner.state) {
        State::Open(_) => {}
        State::Busy { .. } => return true,
        State::Closed | State::Panicked => return false,
    }
    let mut access = Access {
        inner,
        first: true,
        dead: false,
    };
    // `None`: a long call took the store between two steps; the next tick starts afresh.
    let result = fin_store::Store::tick_phased(&mut access, Instant::now(), LIGHT);
    if access.dead {
        return false;
    }
    if let Err(e) = result {
        let e: CoreError = e.into();
        inner.events.emit(error_json(&json!(e), false));
    }
    true
}

/// The store as a tick's steps reach it (module docs): under the state mutex, only while it
/// is `Open`. The events a step queued are forwarded before the mutex is released.
struct Access<'a> {
    inner: &'a Shared,
    /// The first step (where the test panic hook fires).
    first: bool,
    /// A step panicked: the store is dropped and the engine dead.
    dead: bool,
}

impl StoreAccess for Access<'_> {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> Option<R> {
        if self.dead {
            return None;
        }
        let inner = self.inner;
        let first = std::mem::replace(&mut self.first, false);
        let mut guard = lock(&inner.state);
        let State::Open(store) = &mut *guard else {
            return None;
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(feature = "test-hooks")]
            if first && inner.hooks.take_tick_panic() {
                panic!("fin-node test panic in tick");
            }
            let _ = first;
            let r = f(store);
            (r, store.take_events())
        }));
        match result {
            Ok((r, events)) => {
                // Forwarded before the store is released: a commit that runs right after this
                // step answers after these events reach JS (a rewind is seen before the new
                // history's first commit).
                forward(inner, store, &events);
                drop(guard);
                Some(r)
            }
            Err(payload) => {
                let store = std::mem::replace(&mut *guard, State::Panicked);
                drop(guard);
                let _ = catch_unwind(AssertUnwindSafe(move || drop(store)));
                let e = panic_error(payload.as_ref());
                inner.events.emit(error_json(&json!(e), true));
                self.dead = true;
                None
            }
        }
    }
}

/// Hands a tick step's events to the fonts, the journal and JS (under the state mutex).
fn forward(inner: &Shared, store: &Store, events: &[StoreEvent]) {
    if events.is_empty() {
        return;
    }
    let incarnation = events
        .iter()
        .any(|e| matches!(e, StoreEvent::Rewound { .. }))
        .then(|| store.incarnation());
    let mut rescan_all = false;
    for e in events {
        match e {
            StoreEvent::Applied(c) => inner.fonts.committed(store, c),
            StoreEvent::Rewound { .. } | StoreEvent::Rebuilt { .. } => rescan_all = true,
            // The journal learns the flag before JS sees the event.
            StoreEvent::Collect(p) => {
                if let Some(g) = lock(&inner.journal).as_ref() {
                    g.update(*p);
                }
            }
            _ => {}
        }
    }
    if rescan_all {
        inner.fonts.rescan_all(store);
    }
    for e in events {
        if let StoreEvent::Status(st) = e {
            let now = st.read_only.as_ref().map(|r| r.reason.code());
            crate::engine::observe_read_only(inner, now);
        }
        if let StoreEvent::Status(st) = e
            && let Ok(v) = serde_json::to_value(st)
        {
            *lock(&inner.status) = Some(v.clone());
            // The store's status with the font summary merged (Decision 34).
            inner.fonts.emit_status(v, &inner.events);
            continue;
        }
        inner.events.emit(event_json(e, incarnation.as_deref()));
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
