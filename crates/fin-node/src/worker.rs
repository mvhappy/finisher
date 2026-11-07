//! The per-engine worker thread for long calls (plan Decision 7).
//!
//! `create`/`open` (which may import a whole folder or rebuild the database), `takeover`,
//! `verify`, `rebuild`, `compact` and `close` run here one at a time, in call order, and
//! settle a napi deferred promise. They never occupy the libuv pool. So do the daily backup
//! and compaction the maintenance thread finds due ([`auto`]: operations `backup (auto)` and
//! `compaction (auto)`), and the close an engine garbage-collected without `close` sends.
//!
//! A long call takes the store out of the shared state (which then reads `Busy`) and puts it
//! back when done: a short call meanwhile waits up to [`crate::engine::BUSY_WAIT`] and then
//! fails with `ENGINE_BUSY`; `status()` answers at once from the status cached after the last
//! tick or long call, with `busy: {operation, sinceMs}`; the maintenance thread skips its
//! ticks. `close` queues behind running and queued long calls.
//!
//! Cancellation: a running long call is not interrupted (`fin-store` has no cancellation
//! points inside verify, rebuild or compaction; each is crash-safe on its own). A long call
//! that starts after `close` fails with `ENGINE_CLOSED` without touching the store.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use fin_core::{CoreError, Rational};

use fin_store::{OpenOptions, Store};
use napi::bindgen_prelude::*;

use crate::engine::{Engine, Shared, State, lock};
use crate::{guarded, now_ms, panic_error, reject};

type Job = Box<dyn FnOnce() + Send>;

/// The sending end of an engine's worker thread. The thread ends once every `Worker` (the
/// engine's, and those held by queued jobs) is dropped and its queue is empty.
#[derive(Clone)]
pub(crate) struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    pub(crate) fn spawn() -> Result<Worker> {
        let (jobs, queue) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("fin-node-worker".to_string())
            .spawn(move || {
                for job in queue {
                    // Jobs catch their own panics; this only keeps the thread alive.
                    let _ = catch_unwind(AssertUnwindSafe(job));
                }
            })
            .map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("cannot start the engine worker thread: {e}"),
                ))
            })?;
        Ok(Worker { jobs })
    }

    pub(crate) fn submit(&self, job: Job) -> Result<()> {
        self.jobs.send(job).map_err(|_| {
            reject(CoreError::new(
                "INTERNAL",
                "the engine worker thread has stopped",
            ))
        })
    }
}

/// `Engine.create` (`create` = `(name, rate, idem key)`) or `Engine.open` (`relocate`:
/// [`OpenOptions::relocate`]), on a new worker thread that then serves the engine.
pub(crate) fn open<'e>(
    env: &'e Env,
    server_data: PathBuf,
    project_dir: PathBuf,
    create: Option<(String, String, Option<String>)>,
    relocate: bool,
) -> Result<Object<'e>> {
    let worker = Worker::spawn()?;
    let (deferred, promise) = env.create_deferred::<Engine, _>()?;
    let keep = worker.clone();
    worker.submit(Box::new(move || {
        let opened = guarded(
            || {
                // The server's font manager: its shared measurer is what Task 20 passes as
                // `OpenOptions.measurer` (the edit path's overflow check).
                let manager = crate::fonts::for_server(&server_data);
                let store = match &create {
                    Some((name, rate, idem)) => {
                        let rate: Rational = rate.parse().map_err(reject)?;
                        Store::create(&server_data, &project_dir, name, rate, idem.as_deref())
                    }
                    None => Store::open_with(
                        &server_data,
                        &project_dir,
                        OpenOptions {
                            relocate,
                            ..OpenOptions::default()
                        },
                    ),
                };
                let store = store.map_err(reject)?;
                let status = serde_json::to_value(store.status()).ok();
                // Read here, off the JS thread: what every branch pins (Decision 34's open
                // trigger fetches what of it is missing once the engine attaches).
                let fonts = crate::fonts::Opened {
                    manager,
                    branches: crate::fonts::scan::store(&store),
                };
                Ok((store, status, fonts))
            },
            || {},
        );
        match opened {
            Ok((store, status, fonts)) => deferred
                .resolve(move |_env| guarded(|| Engine::start(store, status, keep, fonts), || {})),
            Err(e) => deferred.reject(e),
        }
    }))?;
    Ok(promise)
}

/// Runs `f` on the worker with the store taken out of the shared state (module docs).
pub(crate) fn long<'e>(
    env: &'e Env,
    inner: &Shared,
    worker: &Worker,
    operation: &'static str,
    f: impl FnOnce(&mut Store) -> Result<String> + Send + 'static,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    let inner = inner.clone();
    worker.submit(Box::new(move || match run_long(&inner, operation, f) {
        Ok(s) => deferred.resolve(move |_env| Ok(s)),
        Err(e) => deferred.reject(e),
    }))?;
    Ok(promise)
}

/// Queues the backup or compaction a tick found due (`Store::due_work`) as a long call,
/// unless one is queued already. Its failure is a non-fatal `error` event; a panic, a fatal one.
pub(crate) fn auto(inner: &Shared, worker: &Worker) {
    let due = {
        let g = lock(&inner.state);
        match &*g {
            State::Open(s) => s.due_work(),
            _ => return,
        }
    };
    if !(due.backup || due.compaction) || inner.auto_queued.swap(true, Ordering::SeqCst) {
        return;
    }
    let operation = if due.compaction {
        "compaction (auto)"
    } else {
        "backup (auto)"
    };
    let job_inner = inner.clone();
    let sent = worker.submit(Box::new(move || {
        let inner = job_inner;
        let done = run_long(&inner, operation, |s| {
            let due = s.due_work();
            if due.backup {
                s.run_due_backup();
            }
            if due.compaction {
                s.run_due_compaction().map_err(reject)?;
            }
            Ok(())
        });
        inner.auto_queued.store(false, Ordering::SeqCst);
        if let Err(e) = done {
            let fatal = matches!(*lock(&inner.state), State::Panicked);
            let e = crate::error_value(&e);
            inner.events.emit(crate::maintenance::error_json(&e, fatal));
        }
    }));
    if sent.is_err() {
        inner.auto_queued.store(false, Ordering::SeqCst);
    }
}

fn run_long<T>(
    inner: &Shared,
    operation: &'static str,
    f: impl FnOnce(&mut Store) -> Result<T>,
) -> Result<T> {
    let mut store = {
        let mut g = lock(&inner.state);
        let busy = State::Busy {
            operation,
            since_ms: now_ms(),
        };
        match std::mem::replace(&mut *g, busy) {
            State::Open(store) => store,
            other => {
                let e = other.unavailable();
                *g = other;
                return Err(reject(e));
            }
        }
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(feature = "test-hooks")]
        inner.hooks.long_call_delay();
        let r = f(&mut store);
        // A takeover (its import), rebuild or recovery may have changed any branch's pins.
        if matches!(operation, "takeover" | "rebuild" | "recover") {
            inner.fonts.rescan_all(&store);
        }
        if let Ok(status) = serde_json::to_value(store.status()) {
            *lock(&inner.status) = Some(status);
        }
        r
    }));
    let (back, out) = match result {
        Ok(r) => (State::Open(store), r),
        Err(payload) => {
            let _ = catch_unwind(AssertUnwindSafe(move || drop(store)));
            (State::Panicked, Err(reject(panic_error(payload.as_ref()))))
        }
    };
    let mut g = lock(&inner.state);
    // Only our own `Busy` is replaced: a state set meanwhile (`Panicked`) stays.
    if matches!(*g, State::Busy { .. }) {
        *g = back;
    }
    drop(g);
    inner.returned.notify_all();
    out
}

/// `close` (module docs): stops maintenance, then flushes the export and closes the store.
pub(crate) fn close<'e>(env: &'e Env, inner: &Shared, worker: &Worker) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<(), _>()?;
    let inner = inner.clone();
    worker.submit(Box::new(move || {
        let done = guarded(|| close_now(&inner), || {});
        match done {
            Ok(()) => deferred.resolve(|_env| Ok(())),
            Err(e) => deferred.reject(e),
        }
    }))?;
    Ok(promise)
}

pub(crate) fn close_now(inner: &Shared) -> Result<()> {
    let m = lock(&inner.maintenance).take();
    if let Some(m) = m {
        m.stop();
    }
    let store = {
        let mut g = lock(&inner.state);
        match std::mem::replace(&mut *g, State::Closed) {
            State::Open(store) => Some(store),
            other => {
                *g = other;
                None
            }
        }
    };
    inner.events.clear();
    inner.fonts.detach();
    match store {
        // Folder failures stay unexported (the next open exports); only a database
        // failure is an error here.
        Some(store) => store.close().map(drop).map_err(reject),
        None => Ok(()),
    }
}
