//! The per-engine worker thread for long calls (plan Decision 7).
//!
//! `create`/`open` (which may import a whole folder or rebuild the database), `takeover`,
//! `verify`, `rebuild`, `compact` and `close` run here one at a time, in call order, and
//! settle a napi deferred promise. They never occupy the libuv pool.
//!
//! A long call takes the store out of the shared state (which then reads `Busy`) and puts it
//! back when done: short calls meanwhile fail at once with `ENGINE_BUSY`, and the maintenance
//! thread skips its ticks. `close` queues behind running and queued long calls.
//!
//! Cancellation: a running long call is not interrupted (`fin-store` has no cancellation
//! points inside verify, rebuild or compaction; each is crash-safe on its own). A long call
//! that starts after `close` fails with `ENGINE_CLOSED` without touching the store.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc;

use fin_core::{CoreError, Rational};
use fin_store::{CompactReport, Store};
use napi::bindgen_prelude::*;
use serde_json::json;

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

    fn submit(&self, job: Job) -> Result<()> {
        self.jobs.send(job).map_err(|_| {
            reject(CoreError::new(
                "INTERNAL",
                "the engine worker thread has stopped",
            ))
        })
    }
}

/// `Engine.create` (`create` = `(name, rate)`) or `Engine.open`, on a new worker thread that
/// then serves the engine.
pub(crate) fn open<'e>(
    env: &'e Env,
    server_data: PathBuf,
    project_dir: PathBuf,
    create: Option<(String, String)>,
) -> Result<Object<'e>> {
    let worker = Worker::spawn()?;
    let (deferred, promise) = env.create_deferred::<Engine, _>()?;
    let keep = worker.clone();
    worker.submit(Box::new(move || {
        let opened = guarded(
            || {
                let store = match &create {
                    Some((name, rate)) => {
                        let rate: Rational = rate.parse().map_err(reject)?;
                        Store::create(&server_data, &project_dir, name, rate)
                    }
                    None => Store::open(&server_data, &project_dir),
                };
                store.map_err(reject)
            },
            || {},
        );
        match opened {
            Ok(store) => {
                deferred.resolve(move |_env| guarded(|| Engine::start(store, keep), || {}))
            }
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
        f(&mut store)
    }));
    match result {
        Ok(r) => {
            *lock(&inner.state) = State::Open(store);
            r
        }
        Err(payload) => {
            *lock(&inner.state) = State::Panicked;
            let _ = catch_unwind(AssertUnwindSafe(move || drop(store)));
            Err(reject(panic_error(payload.as_ref())))
        }
    }
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

fn close_now(inner: &Shared) -> Result<()> {
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
    match store {
        // Folder failures stay unexported (the next open exports); only a database
        // failure is an error here.
        Some(store) => store.close().map(drop).map_err(reject),
        None => Ok(()),
    }
}

/// `CompactReport` as JSON (camelCase, like the store's other reports).
pub(crate) fn compact_json(r: &CompactReport) -> serde_json::Value {
    let pairs = |v: &[(String, String)]| -> Vec<serde_json::Value> {
        v.iter()
            .map(|(name, why)| json!({ "name": name, "why": why }))
            .collect()
    };
    json!({
        "skipped": r.skipped,
        "manifests": r.manifests,
        "packsWritten": r.packs_written,
        "replaced": r.replaced,
        "finished": r.finished,
        "deleted": r.deleted,
        "waiting": pairs(&r.waiting),
        "abandoned": pairs(&r.abandoned),
    })
}
