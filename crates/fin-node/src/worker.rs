//! The per-engine worker thread for long calls (plan Decision 7).
//!
//! `create`/`open` (which may import a whole folder or rebuild the database), `takeover`,
//! `verify`, `rebuild`, `compact` and `close` run here one at a time, in call order, and
//! settle a napi deferred promise. They never occupy the libuv pool. So does the close an
//! engine garbage-collected without `close` sends. Automatic maintenance is not a long call
//! (`crate::auto`): a long call cancels a running automatic backup and waits for it to stop.
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
        Worker::spawn_named("fin-node-worker")
    }

    /// A worker thread called `name` (the media cache's own thread is `fin-node-cache`).
    pub(crate) fn spawn_named(name: &str) -> Result<Worker> {
        let (jobs, queue) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                for job in queue {
                    // Jobs catch their own panics; this only keeps the thread alive.
                    let _ = catch_unwind(AssertUnwindSafe(job));
                }
            })
            .map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("cannot start the {name} thread: {e}"),
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
    expect_id: Option<String>,
) -> Result<Object<'e>> {
    let worker = Worker::spawn()?;
    let (deferred, promise) = env.create_deferred::<Engine, _>()?;
    let keep = worker.clone();
    worker.submit(Box::new(move || {
        let opened = guarded(
            || {
                // The server's font manager: its shared measurer is the store's, so edits,
                // history writes and recovery check caption overflow (plan 4C Task 20).
                let manager = crate::fonts::for_server(&server_data);
                let measurer = Some(manager.measurer());
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
                            measurer: measurer.clone(),
                            expect_id,
                            ..OpenOptions::default()
                        },
                    ),
                };
                let mut store = store.map_err(reject)?;
                store.set_measurer(measurer);
                let status = serde_json::to_value(store.status()).ok();
                // Read here, off the JS thread: what every branch pins, its files hashed and
                // what of it is missing queued (Decision 34's open trigger).
                let fonts = crate::fonts::Opened::read(manager, &store);
                // The project's flag, before anything is recorded for it (unknown drops).
                let diag = crate::journal::for_server(&server_data);
                let journal = diag.open_project(store.project_id(), store.collect_policy());
                Ok((store, status, fonts, (diag, journal)))
            },
            || {},
        );
        match opened {
            Ok((store, status, fonts, journal)) => deferred.resolve(move |_env| {
                guarded(|| Engine::start(store, status, keep, fonts, journal), || {})
            }),
            Err(e) => {
                let call = if create.is_some() {
                    "project.create"
                } else {
                    "project.open"
                };
                let j = crate::journal::for_server(&server_data);
                crate::commit_log::internal(&j, None, call, &crate::error_value(&e));
                deferred.reject(e)
            }
        }
    }))?;
    Ok(promise)
}

/// Runs `f` on the worker thread, queued behind the long calls, without taking the store out
/// of the shared state: `f` reads what it needs under the mutex and works on its copy (plan
/// 6A Task 12: the media job inputs). Resolves to `f`'s JSON.
pub(crate) fn offload<'e>(
    env: &'e Env,
    worker: &Worker,
    f: impl FnOnce() -> Result<String> + Send + 'static,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    worker.submit(Box::new(move || match guarded(f, || {}) {
        Ok(s) => deferred.resolve(move |_env| Ok(s)),
        Err(e) => deferred.reject(e),
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
        Err(e) => {
            let call = format!("project.{operation}");
            let value = crate::error_value(&e);
            crate::commit_log::internal(&inner.diag, Some(&inner.project), &call, &value);
            deferred.reject(e)
        }
    }))?;
    Ok(promise)
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
    // After `Busy` is set: no new automatic backup starts until the store is back.
    if let Some((job, outcome)) = crate::auto::stop_backup(inner) {
        store.finish_backup(&job, &outcome);
    }
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
        // A takeover or a recovery may lift (or cause) the read-only state.
        crate::engine::observe_read_only(inner, store.read_only().map(|r| r.code()));
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
    // Maintenance has stopped, so no backup starts after this one.
    drop(crate::auto::stop_backup(inner));
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
    drop(lock(&inner.journal).take());
    match store {
        // Folder failures stay unexported (the next open exports); only a database
        // failure is an error here.
        Some(store) => store.close().map(drop).map_err(reject),
        None => Ok(()),
    }
}
