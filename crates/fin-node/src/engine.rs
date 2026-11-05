//! The `Engine` class: one project's store, its worker thread and its maintenance thread.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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

/// How long a short call waits for a long call to give the store back before it fails with
/// `ENGINE_BUSY`: long enough for an automatic backup or compaction of a small project, short
/// enough not to park a libuv thread.
pub(crate) const BUSY_WAIT: Duration = Duration::from_millis(250);

/// What an `Engine`, its in-flight calls and its threads share.
pub(crate) struct Inner {
    pub(crate) state: Mutex<State>,
    /// Notified whenever the state stops being `Busy`.
    pub(crate) returned: Condvar,
    /// The last `ProjectStatus` seen (JSON): what `status()` answers during a long call.
    pub(crate) status: Mutex<Option<serde_json::Value>>,
    /// An automatic backup or compaction is queued on the worker (or running).
    pub(crate) auto_queued: AtomicBool,
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

/// The state, after waiting up to [`BUSY_WAIT`] for a long call to give the store back.
pub(crate) fn lock_state(shared: &Shared, wait: Duration) -> MutexGuard<'_, State> {
    let deadline = Instant::now() + wait;
    let mut guard = lock(&shared.state);
    while matches!(*guard, State::Busy { .. }) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        guard = shared
            .returned
            .wait_timeout(guard, left)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
    guard
}

/// Runs `f` against the open store, or fails with `ENGINE_CLOSED` / `ENGINE_BUSY`.
pub(crate) fn with_store<T>(shared: &Shared, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let mut guard = lock_state(shared, BUSY_WAIT);
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
    pub(crate) fn start(
        store: Store,
        status: Option<serde_json::Value>,
        worker: Worker,
    ) -> Result<Engine> {
        let project_id = store.project_id().to_string();
        let inner = Arc::new(Inner {
            state: Mutex::new(State::Open(Box::new(store))),
            returned: Condvar::new(),
            status: Mutex::new(status),
            auto_queued: AtomicBool::new(false),
            events: EventSink::default(),
            maintenance: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            hooks: Default::default(),
        });
        let m = Maintenance::start(&inner, worker.clone())?;
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
    /// Garbage collected without `close`: a fire-and-forget close goes to the worker, which
    /// stops the maintenance thread and flushes off the JS thread, after queued long calls.
    fn drop(&mut self) {
        let inner = self.inner.clone();
        let sent = self.worker.submit(Box::new(move || {
            let _ = crate::guarded(|| worker::close_now(&inner), || {});
        }));
        if sent.is_err()
            && let Some(m) = lock(&self.inner.maintenance).take()
        {
            m.signal();
        }
    }
}

#[napi]
impl Engine {
    /// Creates a project in `project_dir`, its database under `server_data`. `rate` is a
    /// rational string such as `"30"` or `"30000/1001"`; `idem_key`: a retry with it returns
    /// the project this create made (`Store::create`). Resolves to an `Engine`.
    #[napi(catch_unwind, ts_return_type = "Promise<Engine>")]
    pub fn create<'e>(
        env: &'e Env,
        server_data: String,
        project_dir: String,
        name: String,
        rate: String,
        idem_key: Option<String>,
    ) -> Result<Object<'e>> {
        worker::open(
            env,
            server_data.into(),
            project_dir.into(),
            Some((name, rate, idem_key)),
            false,
        )
    }

    /// Opens the project whose `finisher.project.json` is in `project_dir` (acquiring the
    /// lease or following, importing, rebuilding a missing or damaged database). `relocate`:
    /// the user says the project now lives here although its recorded folder cannot be
    /// reached (`PROJECT_ORIGINAL_UNREACHABLE` otherwise).
    #[napi(catch_unwind, ts_return_type = "Promise<Engine>")]
    pub fn open<'e>(
        env: &'e Env,
        server_data: String,
        project_dir: String,
        relocate: Option<bool>,
    ) -> Result<Object<'e>> {
        worker::open(
            env,
            server_data.into(),
            project_dir.into(),
            None,
            relocate.unwrap_or(false),
        )
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

    /// Resolves to `{"rev": n, "project": {...}, "generation": {incarnation, n}}` JSON for the
    /// head of `branch`, read together (the generation to commit against what was read).
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
            encode(&s.compact(true).map_err(reject)?)
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
