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
    /// This engine's place in its server's font manager (pins, status, the shared measurer).
    pub(crate) fonts: crate::fonts::Handle,
    /// The project's flag in the server's journal, until `close` (unknown afterwards).
    pub(crate) journal: Mutex<Option<crate::journal::ProjectGuard>>,
    /// The server's journal (found at open): `log.commit`, `storage.state`, `eval.stats`
    /// and `error.internal` go there ([`crate::commit_log`]).
    pub(crate) diag: fin_journal::Journal,
    /// The project id (lower case), for the entries' `ctx.project`.
    pub(crate) project: String,
    /// The read-only reason last seen (its code), for `storage.state` on a change.
    pub(crate) read_only: Mutex<Option<&'static str>>,
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

/// Records `storage.state` when the project's read-only reason (`now`, a `ReadOnlyReason`
/// code) differs from the last one seen (status events, long calls).
pub(crate) fn observe_read_only(shared: &Shared, now: Option<&'static str>) {
    let mut seen = lock(&shared.read_only);
    if *seen != now {
        crate::commit_log::storage_state(&shared.diag, &shared.project, *seen, now);
        *seen = now;
    }
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
        fonts: crate::fonts::Opened,
        (diag, journal): (fin_journal::Journal, crate::journal::ProjectGuard),
    ) -> Result<Engine> {
        let project_id = store.project_id().to_string();
        let read_only = store.read_only().map(fin_store::ReadOnlyReason::code);
        let inner = Arc::new_cyclic(|weak| Inner {
            fonts: fonts
                .manager
                .attach(project_id.clone(), weak.clone(), fonts.branches),
            state: Mutex::new(State::Open(Box::new(store))),
            returned: Condvar::new(),
            status: Mutex::new(status),
            auto_queued: AtomicBool::new(false),
            events: EventSink::default(),
            maintenance: Mutex::new(None),
            journal: Mutex::new(Some(journal)),
            diag,
            project: project_id.clone(),
            read_only: Mutex::new(read_only),
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

    pub(crate) fn offload<'e>(
        &self,
        env: &'e Env,
        f: impl FnOnce() -> Result<String> + Send + 'static,
    ) -> Result<Object<'e>> {
        worker::offload(env, &self.worker, f)
    }

    pub(crate) fn long<'e>(
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
            None,
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
        expect_id: Option<String>,
    ) -> Result<Object<'e>> {
        worker::open(
            env,
            server_data.into(),
            project_dir.into(),
            None,
            relocate.unwrap_or(false),
            expect_id,
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
    /// `meta_json` (`{actor?: {kind, session?, call?}, guard?: WriteGuard}`): `actor`
    /// (finisherd's `callMeta`) is the actor of the `log.commit` entry (`unknown` without
    /// it); `guard` (`{denyBranches?, allowBranches?}`, branch ids) is checked inside the
    /// write transaction against the branch the tx resolves to (`BRANCH_PROTECTED`,
    /// `BRANCH_NOT_ALLOWED`). A `meta_json` or `guard` that does not decode is `INVALID_OP`.
    #[napi(catch_unwind)]
    pub fn commit(
        &self,
        tx_json: String,
        generation_json: Option<String>,
        meta_json: Option<String>,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::Commit {
            tx_json,
            generation_json,
            meta_json,
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
    /// `false` when no such rewind is recorded. `meta_json`'s guard names the credential, if
    /// any, refused once ended (`CREDENTIAL_REVOKED`), as a write's is.
    #[napi(catch_unwind)]
    pub fn acknowledge_rewind(
        &self,
        branch: String,
        generation: f64,
        meta_json: Option<String>,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::AcknowledgeRewind {
            branch,
            generation,
            meta_json,
        })
    }

    /// Acknowledges an incomplete salvage of the kept copy `copy`; `false` when none is listed.
    /// `meta_json` as for `acknowledge_rewind`.
    #[napi(catch_unwind)]
    pub fn acknowledge_salvage(
        &self,
        copy: String,
        meta_json: Option<String>,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::AcknowledgeSalvage { copy, meta_json })
    }

    /// Takes the lease over; resolves to the `ProjectStatus` JSON (long call), with the font
    /// summary merged as `status()` has it.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn takeover<'e>(&self, env: &'e Env, confirm: bool) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        self.long(env, "takeover", move |s| {
            let status = s.takeover(confirm).map_err(reject)?;
            let raw = serde_json::to_value(&status).unwrap_or(serde_json::Value::Null);
            encode(&inner.fonts.merged(raw))
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

    /// Sets the project's collection flag (`default` or `never`) in `finisher.project.json`
    /// (needs the writer lease: `PROJECT_READ_ONLY` otherwise) and in the server's journal;
    /// `never` purges the project's entries. Resolves to `{"project", "collect"}` JSON.
    /// `meta_json` as for `acknowledge_rewind`.
    #[napi(catch_unwind)]
    pub fn set_collect(&self, collect: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::SetCollect { collect, meta_json })
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
