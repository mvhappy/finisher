//! napi-rs binding exposing `fin-store` and `fin-probe` to Node as `@finisher/fin-node`.
//!
//! # Threads
//!
//! No call runs on the JS thread. Values cross the boundary as JSON strings.
//! - **Short calls** (`head`, `commit`, `snapshot`, `opsSince`, `status`, `txGet`, the
//!   acknowledgements, the branch and history calls of [`history`]) are napi `AsyncTask`s on the libuv pool ([`tasks`]).
//! - **Long calls** (`create`/`open`, which may import or rebuild everything; `takeover`,
//!   `verify`, `rebuild`, `compact`, `recover`; `close`, which flushes the export) run one at a time on a
//!   dedicated per-engine worker thread and settle a napi deferred promise ([`worker`], plan
//!   Decision 7), so they never hold a libuv thread for seconds.
//! - **Maintenance**: one thread per engine calls `Store::tick` every 500 ms and forwards
//!   `StoreEvent`s to the `onEvent` callback through an unref'd threadsafe function
//!   ([`maintenance`]).
//!
//! All three share the store through one mutex ([`engine::Inner`]). A long call takes the
//! store out of it for its duration (the state reads `Busy`), so short calls during a long call
//! fail at once with `ENGINE_BUSY` (transient, exit 5) instead of occupying the libuv pool, and
//! ticks are skipped. A tick holds the mutex for its duration: usually milliseconds, longer when
//! it takes the daily backup or compacts; a short call then waits for it.
//!
//! # Errors and panics
//!
//! A failure rejects with a napi error whose message is `ERROR_PREFIX` followed by the
//! `CoreError` JSON (`{code, message, details, fix?}`); the JS wrapper in
//! `packages/fin-node/index.js` turns that into an `Error` carrying those fields.
//!
//! napi runs `Task::compute` inside an `extern "C"` callback without `catch_unwind`, so a panic
//! there would abort the whole Node process. Every task, worker job and tick therefore runs
//! under `catch_unwind`, which turns a panic into an `INTERNAL` rejection (or a fatal `error`
//! event, for a tick); an `Engine` whose store panicked is dead (`ENGINE_CLOSED`,
//! `details.reason = "panicked"`), since the store may be inconsistent.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

mod engine;
mod history;
mod maintenance;
mod server_lock;
mod tasks;
#[cfg(feature = "test-hooks")]
mod test_hooks;
mod worker;

pub use engine::Engine;
pub use server_lock::{ServerLock, lock_exclusive};
pub use tasks::StoreTask;

/// Marks a napi error message as a serialized `CoreError`. Must match `index.js`.
const ERROR_PREFIX: &str = "FIN_CORE_ERROR:";

fn reject(e: impl Into<CoreError>) -> Error {
    let e: CoreError = e.into();
    let body = serde_json::to_string(&e).unwrap_or_else(|_| {
        json!({ "code": "INTERNAL", "message": e.message, "details": {} }).to_string()
    });
    Error::new(Status::GenericFailure, format!("{ERROR_PREFIX}{body}"))
}

/// The `CoreError` JSON a rejection made by [`reject`] carries (else an `INTERNAL` one).
fn error_value(e: &Error) -> serde_json::Value {
    e.reason
        .strip_prefix(ERROR_PREFIX)
        .and_then(|body| serde_json::from_str(body).ok())
        .unwrap_or_else(|| json!({ "code": "INTERNAL", "message": e.reason, "details": {} }))
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

/// Unix ms now.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Probes an MP4/MOV file; resolves to `Probe` JSON (`{duration, video, audio, fingerprint,
/// size}`).
#[napi(catch_unwind)]
pub fn probe(path: String) -> AsyncTask<ProbeTask> {
    AsyncTask::new(ProbeTask { path: path.into() })
}

pub struct ProbeTask {
    path: PathBuf,
}

impl Task for ProbeTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<String> {
        let path = &self.path;
        guarded(
            // `Probe`'s serde form: the `Probe` type of `@finisher/core-types`.
            || encode(&fin_probe::probe(path).map_err(reject)?),
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
