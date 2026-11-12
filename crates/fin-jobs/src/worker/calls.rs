//! Host calls (Decision 5a): `call {id, method, params}` out, `reply {id, value | error}`
//! back, routed by `id` to the thread that waits for it. Replies may come in any order. The
//! host limits calls in flight (16; the 17th is answered `CALL_LIMIT`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::Duration;

use serde_json::Value;

use crate::error::JobError;
use crate::msg_worker::WorkerMsg;
use crate::worker::ctx::CancelToken;
use crate::worker::io::Out;

type Answer = Result<Value, JobError>;

/// How often a cancel-aware call looks at its token while it waits.
const CANCEL_POLL: Duration = Duration::from_millis(10);

/// The calls waiting for a reply.
#[derive(Default)]
pub(crate) struct Calls {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, SyncSender<Answer>>>,
    gone: AtomicBool,
}

fn host_gone() -> JobError {
    JobError::new("HOST_GONE", "the host is gone; the call has no answer")
}

impl Calls {
    /// Sends `call` and blocks this thread until its reply.
    pub(crate) fn call(&self, out: &Out, method: &str, params: Value) -> Answer {
        let (_, rx) = self.send(out, method, params)?;
        rx.recv().unwrap_or_else(|_| Err(host_gone()))
    }

    /// [`Calls::call`] that gives up on a cancel: a call after `token` is set is not sent, and
    /// one in flight fails [`JobError::cancelled`] within [`CANCEL_POLL`] of it (its reply,
    /// when it comes, is dropped).
    pub(crate) fn call_until(
        &self,
        out: &Out,
        method: &str,
        params: Value,
        token: &CancelToken,
    ) -> Answer {
        if token.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let (id, rx) = self.send(out, method, params)?;
        loop {
            match rx.recv_timeout(CANCEL_POLL) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Disconnected) => return Err(host_gone()),
                Err(RecvTimeoutError::Timeout) if token.is_cancelled() => {
                    self.forget(id);
                    return Err(JobError::cancelled());
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Sends `call` and does not wait: its reply, whenever it comes, is dropped (fire and
    /// forget, the plugin SDK's `log`). A call that cannot be sent is dropped too.
    pub(crate) fn call_detached(&self, out: &Out, method: &str, params: Value) {
        if let Err(e) = self.send(out, method, params) {
            eprintln!("fin-jobs: a {method} call was not sent: {}", e.message);
        }
    }

    /// Sends a message of a type the host may not know, with a fresh `id`, and waits for its
    /// reply (test-hooks: `test.pluginMinor`).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn call_raw(&self, out: &Out, t: &str) -> Answer {
        let (id, rx) = self.open()?;
        out.send_value(
            |seq| serde_json::json!({ "v": crate::PROTOCOL, "t": t, "seq": seq, "id": id }),
        );
        rx.recv().unwrap_or_else(|_| Err(host_gone()))
    }

    /// Opens a call and sends it; `CALL_TOO_LARGE` when its line passes the cap.
    fn send(
        &self,
        out: &Out,
        method: &str,
        params: Value,
    ) -> Result<(u64, mpsc::Receiver<Answer>), JobError> {
        let (id, rx) = self.open()?;
        let msg = WorkerMsg::Call {
            id,
            method: method.to_owned(),
            params,
        };
        if let Err(e) = out.send(&msg) {
            self.forget(id);
            return Err(JobError::new("CALL_TOO_LARGE", e.to_string())
                .with_details(serde_json::json!({ "method": method })));
        }
        Ok((id, rx))
    }

    fn open(&self) -> Result<(u64, mpsc::Receiver<Answer>), JobError> {
        let mut pending = self.lock();
        if self.gone.load(Ordering::Relaxed) {
            return Err(host_gone());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::sync_channel(1);
        pending.insert(id, tx);
        Ok((id, rx))
    }

    fn forget(&self, id: u64) {
        self.lock().remove(&id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, SyncSender<Answer>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hands a reply to its caller; a reply nobody waits for is dropped.
    pub(crate) fn resolve(&self, id: u64, value: Option<Value>, error: Option<JobError>) {
        let Some(tx) = self.lock().remove(&id) else {
            eprintln!("fin-jobs: a reply to unknown call {id}");
            return;
        };
        let answer = match error {
            Some(e) => Err(e),
            None => Ok(value.unwrap_or(Value::Null)),
        };
        let _ = tx.send(answer);
    }

    /// The host is gone: every waiting call fails `HOST_GONE`, and so does every later one.
    pub(crate) fn fail_all(&self) {
        let mut pending = self.lock();
        self.gone.store(true, Ordering::Relaxed);
        pending.clear();
    }
}
