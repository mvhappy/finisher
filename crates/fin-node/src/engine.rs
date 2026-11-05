//! The `Engine` class: one project's store, its worker thread and its maintenance thread.

use std::sync::{Arc, Mutex, MutexGuard};

use fin_core::CoreError;
use fin_store::Store;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

use crate::maintenance::{EventFn, EventSink, Maintenance};
use crate::tasks::{Call, StoreTask};
use crate::worker::{self, Worker};
use crate::{encode, reject};

/// The store behind an `Engine`.
pub(crate) enum State {
    Open(Box<Store>),
    /// A long call has the store (`operation`, since unix ms `since_ms`); it puts it back.
    Busy {
        operation: &'static str,
        since_ms: i64,
    },
    Closed,
    /// A call panicked while using the store; it is dropped and the engine stays unusable.
    Panicked,
}

impl State {
    /// Why the store is not available (`ENGINE_BUSY` or `ENGINE_CLOSED`).
    pub(crate) fn unavailable(&self) -> CoreError {
        match self {
            State::Busy {
                operation,
                since_ms,
            } => busy(operation, *since_ms),
            State::Panicked => closed("panicked"),
            State::Open(_) | State::Closed => closed("closed"),
        }
    }
}

/// What an `Engine`, its in-flight calls and its threads share.
pub(crate) struct Inner {
    pub(crate) state: Mutex<State>,
    pub(crate) events: EventSink,
    /// The maintenance thread, until `close` (or the engine's drop) stops it.
    pub(crate) maintenance: Mutex<Option<Maintenance>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) hooks: crate::test_hooks::Hooks,
}

pub(crate) type Shared = Arc<Inner>;

/// Locks `m`, ignoring poison. The state mutex is only poisoned by a panic during a call, and
/// every caller then replaces the state with `Panicked` (dropping the store), so the recovered
/// guard never exposes a store that was in use when the panic happened.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Runs `f` against the open store, or fails with `ENGINE_CLOSED` / `ENGINE_BUSY`.
pub(crate) fn with_store<T>(shared: &Shared, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let mut guard = lock(&shared.state);
    match &mut *guard {
        State::Open(store) => f(store),
        other => Err(reject(other.unavailable())),
    }
}

pub(crate) fn closed(reason: &str) -> CoreError {
    let message = match reason {
        "panicked" => "the engine stopped after an internal error",
        _ => "the engine has been closed",
    };
    CoreError::new("ENGINE_CLOSED", message)
        .with_details(json!({ "reason": reason }))
        .with_fix("retry the command; the project reopens automatically")
}

fn busy(operation: &str, since_ms: i64) -> CoreError {
    CoreError::new(
        "ENGINE_BUSY",
        format!("the project is busy: {operation} is running"),
    )
    .with_details(json!({ "operation": operation, "sinceMs": since_ms }))
    .with_fix(format!("retry once the {operation} finishes"))
}

/// A project's op log. Created only through `Engine.create` / `Engine.open`.
#[napi]
pub struct Engine {
    pub(crate) inner: Shared,
    worker: Worker,
    project_id: String,
}

impl Engine {
    /// Wraps an opened store and starts its maintenance thread (on the JS thread, when the
    /// open resolves: an engine JS never receives never ticks).
    pub(crate) fn start(store: Store, worker: Worker) -> Result<Engine> {
        let project_id = store.project_id().to_string();
        let inner = Arc::new(Inner {
            state: Mutex::new(State::Open(Box::new(store))),
            events: EventSink::default(),
            maintenance: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            hooks: Default::default(),
        });
        let m = Maintenance::start(&inner)?;
        *lock(&inner.maintenance) = Some(m);
        Ok(Engine {
            inner,
            worker,
            project_id,
        })
    }

    pub(crate) fn task(&self, call: Call) -> AsyncTask<StoreTask> {
        AsyncTask::new(StoreTask::new(self.inner.clone(), call))
    }

    fn long<'e>(
        &self,
        env: &'e Env,
        operation: &'static str,
        f: impl FnOnce(&mut Store) -> Result<String> + Send + 'static,
    ) -> Result<Object<'e>> {
        worker::long(env, &self.inner, &self.worker, operation, f)
    }
}

impl Drop for Engine {
    /// Garbage collected without `close`: the maintenance thread stops (waiting for a running
    /// tick); the store is dropped, unflushed, once in-flight calls finish.
    fn drop(&mut self) {
        let m = lock(&self.inner.maintenance).take();
        if let Some(m) = m {
            m.stop();
        }
    }
}

#[napi]
impl Engine {
    /// Creates a project in `project_dir`, its database under `server_data`. `rate` is a
    /// rational string such as `"30"` or `"30000/1001"`. Resolves to an `Engine`.
    #[napi(catch_unwind, ts_return_type = "Promise<Engine>")]
    pub fn create<'e>(
        env: &'e Env,
        server_data: String,
        project_dir: String,
        name: String,
        rate: String,
    ) -> Result<Object<'e>> {
        worker::open(
            env,
            server_data.into(),
            project_dir.into(),
            Some((name, rate)),
        )
    }

    /// Opens the project whose `finisher.project.json` is in `project_dir` (acquiring the
    /// lease or following, importing, rebuilding a missing or damaged database).
    #[napi(catch_unwind, ts_return_type = "Promise<Engine>")]
    pub fn open<'e>(env: &'e Env, server_data: String, project_dir: String) -> Result<Object<'e>> {
        worker::open(env, server_data.into(), project_dir.into(), None)
    }

    #[napi(getter, catch_unwind)]
    pub fn project_id(&self) -> String {
        self.project_id.clone()
    }

    /// Head revision of `branch` (0 when the branch does not exist).
    #[napi(catch_unwind)]
    pub fn head(&self, branch: String) -> AsyncTask<StoreTask> {
        self.task(Call::Head { branch })
    }

    /// Commits a `Tx` (JSON); resolves to the `CommittedTx` JSON. With `generation_json`
    /// (`{incarnation, n}`, as `opsSinceAt` or `status` gave it) the commit is refused with
    /// `HISTORY_REWOUND` when the branch was rewound or the database rebuilt since.
    #[napi(catch_unwind)]
    pub fn commit(&self, tx_json: String, generation_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Commit {
            tx_json,
            generation_json,
        })
    }

    /// Resolves to `{"rev": n, "project": {...}}` JSON for the head of `branch`.
    #[napi(catch_unwind)]
    pub fn snapshot(&self, branch: String) -> AsyncTask<StoreTask> {
        self.task(Call::Snapshot { branch })
    }

    /// Resolves to the JSON array of `CommittedTx` on `branch` with `rev > after`.
    #[napi(catch_unwind)]
    pub fn ops_since(&self, branch: String, rev: f64) -> AsyncTask<StoreTask> {
        self.task(Call::OpsSince { branch, after: rev })
    }

    /// Resolves to `{"generation": {incarnation, n}, "ops": CommittedTx[]}` JSON, read together.
    #[napi(catch_unwind)]
    pub fn ops_since_at(&self, branch: String, rev: f64) -> AsyncTask<StoreTask> {
        self.task(Call::OpsSinceAt { branch, after: rev })
    }

    /// Resolves to the `ProjectStatus` JSON.
    #[napi(catch_unwind)]
    pub fn status(&self) -> AsyncTask<StoreTask> {
        self.task(Call::Status)
    }

    /// Resolves to the `CommittedTx` JSON of `tx_id`, `{"setAside": {...}}` JSON when it was
    /// set aside, or `null` when it is not known here.
    #[napi(catch_unwind)]
    pub fn tx_get(&self, tx_id: String) -> AsyncTask<StoreTask> {
        self.task(Call::TxGet { tx_id })
    }

    /// Stops status reporting the rewind of `branch` that started generation `n`; resolves to
    /// `false` when no such rewind is recorded.
    #[napi(catch_unwind)]
    pub fn acknowledge_rewind(&self, branch: String, generation: f64) -> AsyncTask<StoreTask> {
        self.task(Call::AcknowledgeRewind { branch, generation })
    }

    /// Acknowledges an incomplete salvage of the kept copy `copy`; `false` when none is listed.
    #[napi(catch_unwind)]
    pub fn acknowledge_salvage(&self, copy: String) -> AsyncTask<StoreTask> {
        self.task(Call::AcknowledgeSalvage { copy })
    }

    /// Takes the lease over; resolves to the `ProjectStatus` JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn takeover<'e>(&self, env: &'e Env, confirm: bool) -> Result<Object<'e>> {
        self.long(env, "takeover", move |s| {
            encode(&s.takeover(confirm).map_err(reject)?)
        })
    }

    /// Resolves to the `VerifyReport` JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn verify<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        self.long(env, "verify", |s| encode(&s.verify()))
    }

    /// Rebuilds the database from the folder; resolves to the `RebuildReport` JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn rebuild<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        self.long(env, "rebuild", |s| encode(&s.rebuild().map_err(reject)?))
    }

    /// Compacts now (forced: a full scan, no waiting for the trigger); resolves to the report
    /// JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn compact<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        self.long(env, "compact", |s| {
            encode(&worker::compact_json(&s.compact(true).map_err(reject)?))
        })
    }

    /// Delivers every later store event (JSON string) to `callback`, replacing an earlier one.
    /// Events from before the first registration are buffered (the newest 1024). The
    /// callback does not keep the process alive.
    #[napi(catch_unwind)]
    pub fn on_event(
        &self,
        #[napi(ts_arg_type = "(json: string) => void")] callback: Function<String, ()>,
    ) -> Result<()> {
        let tsfn: EventFn = callback
            .build_threadsafe_function::<String>()
            .weak::<true>()
            .build()?;
        self.inner.events.set(tsfn);
        Ok(())
    }

    /// Stops maintenance, flushes the export and closes the database once in-flight calls
    /// finish (after any long call queued before it). Later calls fail with `ENGINE_CLOSED`;
    /// closing twice (or after a panic) is a no-op.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn close<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        worker::close(env, &self.inner, &self.worker)
    }
}
