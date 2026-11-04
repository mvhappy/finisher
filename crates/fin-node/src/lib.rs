//! napi-rs binding exposing `fin-store` and `fin-probe` to Node as `@finisher/fin-node`.
//!
//! Every call runs on the libuv thread pool (`AsyncTask`), never on the JS thread. Values cross
//! the boundary as JSON strings. A failure rejects with a napi error whose message is
//! `ERROR_PREFIX` followed by the `CoreError` JSON (`{code, message, details, fix?}`); the JS
//! wrapper in `packages/fin-node/index.js` turns that into an `Error` carrying those fields.
//!
//! napi runs `Task::compute` inside an `extern "C"` callback without `catch_unwind`, so a panic
//! there would abort the whole Node process. Every task body therefore runs under `guarded`,
//! which turns a panic into an `INTERNAL` rejection; an `Engine` whose store panicked is dead
//! (`ENGINE_CLOSED`, `details.reason = "panicked"`), since the store may be inconsistent.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use fin_core::{CoreError, Rational, Tx};
use fin_probe::Probe;
use fin_store::Store;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

mod server_lock;
#[cfg(feature = "test-hooks")]
mod test_hooks;

pub use server_lock::{ServerLock, lock_exclusive};

/// Marks a napi error message as a serialized `CoreError`. Must match `index.js`.
const ERROR_PREFIX: &str = "FIN_CORE_ERROR:";

fn reject(e: impl Into<CoreError>) -> Error {
    let e: CoreError = e.into();
    let body = serde_json::to_string(&e).unwrap_or_else(|_| {
        json!({ "code": "INTERNAL", "message": e.message, "details": {} }).to_string()
    });
    Error::new(Status::GenericFailure, format!("{ERROR_PREFIX}{body}"))
}

fn encode(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|e| {
        reject(CoreError::new(
            "INTERNAL",
            format!("cannot encode result: {e}"),
        ))
    })
}

fn panic_error(payload: &(dyn Any + Send)) -> CoreError {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    let details = match text {
        Some(t) => json!({ "panic": t }),
        None => json!({}),
    };
    CoreError::new("INTERNAL", "internal error in fin-node").with_details(details)
}

/// Runs `f`, turning a panic into an `INTERNAL` rejection. `on_panic` runs after the unwind.
fn guarded<T>(f: impl FnOnce() -> Result<T>, on_panic: impl FnOnce()) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let err = reject(panic_error(payload.as_ref()));
        // Cleanup must not unwind into the FFI boundary either.
        let _ = catch_unwind(AssertUnwindSafe(on_panic));
        Err(err)
    })
}

/// The store behind an `Engine`.
enum State {
    Open(Box<Store>),
    Closed,
    /// A call panicked while using the store; it is dropped and the engine stays unusable.
    Panicked,
}

/// One `State` shared by an `Engine` and its in-flight tasks.
type Shared = Arc<Mutex<State>>;

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    // The mutex is only poisoned by a panic during a call, and `StoreTask` then replaces the
    // state with `Panicked` (dropping the store), so the recovered guard never exposes a store
    // that was in use when the panic happened.
    shared.lock().unwrap_or_else(|p| p.into_inner())
}

/// Runs `f` against the open store, or fails with `ENGINE_CLOSED`.
fn with_store<T>(shared: &Shared, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let mut guard = lock(shared);
    match &mut *guard {
        State::Open(store) => f(store),
        State::Closed => Err(reject(closed("closed"))),
        State::Panicked => Err(reject(closed("panicked"))),
    }
}

fn closed(reason: &str) -> CoreError {
    let message = match reason {
        "panicked" => "the engine stopped after an internal error",
        _ => "the engine has been closed",
    };
    CoreError::new("ENGINE_CLOSED", message)
        .with_details(json!({ "reason": reason }))
        .with_fix("open the project again with Engine.open")
}

/// A project's op log. Created only through `Engine.create` / `Engine.open`.
#[napi]
pub struct Engine {
    store: Shared,
    project_id: String,
}

#[napi]
impl Engine {
    /// Creates a project in `project_dir`, its database under `server_data`. `rate` is a
    /// rational string such as `"30"` or `"30000/1001"`.
    #[napi(catch_unwind)]
    pub fn create(
        server_data: String,
        project_dir: String,
        name: String,
        rate: String,
    ) -> AsyncTask<OpenTask> {
        AsyncTask::new(OpenTask {
            server_data: server_data.into(),
            project_dir: project_dir.into(),
            create: Some((name, rate)),
        })
    }

    /// Opens the project whose `finisher.project.json` is in `project_dir`.
    #[napi(catch_unwind)]
    pub fn open(server_data: String, project_dir: String) -> AsyncTask<OpenTask> {
        AsyncTask::new(OpenTask {
            server_data: server_data.into(),
            project_dir: project_dir.into(),
            create: None,
        })
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

    /// Commits a `Tx` (JSON); resolves to the `CommittedTx` JSON.
    #[napi(catch_unwind)]
    pub fn commit(&self, tx_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Commit { tx_json })
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

    /// Closes the database once in-flight calls finish. Later calls fail with `ENGINE_CLOSED`;
    /// closing twice (or after a panic) is a no-op.
    #[napi(catch_unwind)]
    pub fn close(&self) -> AsyncTask<StoreTask> {
        self.task(Call::Close)
    }

    fn task(&self, call: Call) -> AsyncTask<StoreTask> {
        AsyncTask::new(StoreTask {
            store: self.store.clone(),
            call: Some(call),
        })
    }
}

pub struct OpenTask {
    server_data: PathBuf,
    project_dir: PathBuf,
    /// `(name, rate)` to create a project; `None` to open one.
    create: Option<(String, String)>,
}

impl Task for OpenTask {
    type Output = Store;
    type JsValue = Engine;

    fn compute(&mut self) -> Result<Store> {
        guarded(
            || {
                let store = match &self.create {
                    Some((name, rate)) => {
                        let rate: Rational = rate.parse().map_err(reject)?;
                        Store::create(&self.server_data, &self.project_dir, name, rate)
                    }
                    None => Store::open(&self.server_data, &self.project_dir),
                };
                store.map_err(reject)
            },
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, store: Store) -> Result<Engine> {
        guarded(
            || {
                Ok(Engine {
                    project_id: store.project_id().to_string(),
                    store: Arc::new(Mutex::new(State::Open(Box::new(store)))),
                })
            },
            || {},
        )
    }
}

enum Call {
    Head {
        branch: String,
    },
    Commit {
        tx_json: String,
    },
    Snapshot {
        branch: String,
    },
    OpsSince {
        branch: String,
        after: f64,
    },
    Close,
    /// Test hook: panics while holding the store.
    #[cfg(feature = "test-hooks")]
    Panic,
}

/// What a `StoreTask` hands back to JS.
pub enum Reply {
    Rev(u64),
    Json(String),
    Unit,
}

pub struct StoreTask {
    store: Shared,
    call: Option<Call>,
}

fn parse_tx(json: &str) -> Result<Tx> {
    serde_json::from_str(json).map_err(|e| {
        reject(
            CoreError::new("INVALID_OP", format!("transaction does not decode: {e}"))
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix("send a transaction matching the fin-core Tx schema"),
        )
    })
}

fn parse_rev(rev: f64) -> Result<u64> {
    // 2^53: the largest integer a JS number holds exactly. NaN fails both checks.
    if rev.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&rev) {
        Ok(rev as u64)
    } else {
        Err(reject(
            CoreError::new(
                "INVALID_RANGE",
                format!("rev must be a non-negative integer, got {rev}"),
            )
            .with_details(json!({ "rev": rev.to_string() })),
        ))
    }
}

fn run(store: &Shared, call: Call) -> Result<Reply> {
    match call {
        Call::Close => {
            let mut guard = lock(store);
            if let State::Open(_) = &*guard {
                *guard = State::Closed;
            }
            Ok(Reply::Unit)
        }
        Call::Head { branch } => with_store(store, |s| Ok(Reply::Rev(s.head(&branch)))),
        Call::Commit { tx_json } => {
            let tx = parse_tx(&tx_json)?;
            with_store(store, |s| {
                Ok(Reply::Json(encode(&s.commit(tx).map_err(reject)?)?))
            })
        }
        Call::Snapshot { branch } => with_store(store, |s| {
            let (rev, project) = s.snapshot(&branch).map_err(reject)?;
            Ok(Reply::Json(encode(
                &json!({ "rev": rev, "project": project }),
            )?))
        }),
        Call::OpsSince { branch, after } => {
            let after = parse_rev(after)?;
            with_store(store, |s| {
                Ok(Reply::Json(encode(
                    &s.ops_since(&branch, after).map_err(reject)?,
                )?))
            })
        }
        #[cfg(feature = "test-hooks")]
        Call::Panic => with_store(store, |_| panic!("fin-node test panic")),
    }
}

impl Task for StoreTask {
    type Output = Reply;
    type JsValue = Either3<f64, String, ()>;

    fn compute(&mut self) -> Result<Reply> {
        let Some(call) = self.call.take() else {
            return Err(reject(CoreError::new("INTERNAL", "task ran twice")));
        };
        let store = &self.store;
        guarded(|| run(store, call), || *lock(store) = State::Panicked)
    }

    fn resolve(&mut self, _env: Env, reply: Reply) -> Result<Self::JsValue> {
        Ok(match reply {
            Reply::Rev(rev) => Either3::A(rev as f64),
            Reply::Json(s) => Either3::B(s),
            Reply::Unit => Either3::C(()),
        })
    }
}

/// Probes an MP4/MOV file; resolves to `{duration, video, audio, fingerprint, size}` JSON.
#[napi(catch_unwind)]
pub fn probe(path: String) -> AsyncTask<ProbeTask> {
    AsyncTask::new(ProbeTask { path: path.into() })
}

pub struct ProbeTask {
    path: PathBuf,
}

fn probe_json(p: &Probe) -> serde_json::Value {
    json!({
        "duration": p.duration,
        "video": p.video,
        "audio": p.audio,
        "fingerprint": p.fingerprint,
        "size": p.size,
    })
}

impl Task for ProbeTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<String> {
        let path = &self.path;
        guarded(
            || encode(&probe_json(&fin_probe::probe(path).map_err(reject)?)),
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
