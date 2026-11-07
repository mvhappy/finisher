//! What a family's `run` gets: its files, helpers, directories and limits, the reporting
//! calls (`begin`, `item`, `progress`, `checkpoint`, `diag`), host calls, the cancel token,
//! a [`Reporter`] for other threads and the read budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::Value;

use crate::budget::ReadBudget;
use crate::codec::CodecError;
use crate::error::JobError;
use crate::msg_host::{CancelReason, Dirs, FileGrant, HelperGrant, Limits, Start};
use crate::msg_worker::{Progress, WorkerMsg};
use crate::worker::io::Shared;

/// Set by the host's `cancel` (or, for a helper, `shutdown`). `Clone + Send`.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicU8>);

const NONE: u8 = 0;
const CANCEL: u8 = 1;
const PAUSE: u8 = 2;

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Sets the token. `cancel` always wins: it upgrades a `pause` (a pause whose grace runs
    /// out, then a cancel, ends as a cancel), and a later `pause` never downgrades it.
    pub fn cancel(&self, reason: CancelReason) {
        match reason {
            CancelReason::Cancel => self.0.store(CANCEL, Ordering::Release),
            CancelReason::Pause => {
                let _ = self
                    .0
                    .compare_exchange(NONE, PAUSE, Ordering::AcqRel, Ordering::Acquire);
            }
        }
    }

    pub fn reason(&self) -> Option<CancelReason> {
        match self.0.load(Ordering::Acquire) {
            CANCEL => Some(CancelReason::Cancel),
            PAUSE => Some(CancelReason::Pause),
            _ => None,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.reason().is_some()
    }
}

fn send(shared: &Shared, msg: &WorkerMsg) {
    if let Err(e) = shared.out.send(msg) {
        eprintln!("fin-jobs: a {} message was not sent: {e}", msg.type_name());
    }
}

/// Reports for one job from any thread. `Clone + Send`.
#[derive(Clone)]
pub struct Reporter {
    shared: Arc<Shared>,
    job: String,
}

impl Reporter {
    pub fn begin(&self, item: &str) {
        send(
            &self.shared,
            &WorkerMsg::Begin {
                job: self.job.clone(),
                item: item.to_owned(),
            },
        );
    }

    pub fn item(&self, item: &str, value: Option<Value>) {
        send(
            &self.shared,
            &WorkerMsg::Item {
                job: self.job.clone(),
                item: item.to_owned(),
                value,
                error: None,
            },
        );
    }

    pub fn item_failed(&self, item: &str, error: JobError) {
        send(
            &self.shared,
            &WorkerMsg::Item {
                job: self.job.clone(),
                item: item.to_owned(),
                value: None,
                error: Some(error),
            },
        );
    }

    pub fn progress(&self, mut p: Progress) {
        p.job.clone_from(&self.job);
        send(&self.shared, &WorkerMsg::Progress(p));
    }

    /// A host call; blocks this thread until the reply.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, JobError> {
        self.shared.calls.call(&self.shared.out, method, params)
    }
}

/// The context of one job's `run`.
pub struct JobCtx<'a> {
    start: &'a Start,
    reporter: Reporter,
    token: CancelToken,
    budget: Option<ReadBudget>,
}

impl<'a> JobCtx<'a> {
    pub(crate) fn new(start: &'a Start, shared: &Arc<Shared>, token: CancelToken) -> JobCtx<'a> {
        JobCtx {
            start,
            reporter: Reporter {
                shared: Arc::clone(shared),
                job: start.job.id.clone(),
            },
            token,
            budget: start.limits.read_bytes_per_sec.map(ReadBudget::new),
        }
    }

    pub fn file(&self, name: &str) -> Option<&FileGrant> {
        self.start.files.iter().find(|f| f.name == name)
    }

    pub fn files(&self) -> &[FileGrant] {
        &self.start.files
    }

    pub fn helpers(&self) -> &[HelperGrant] {
        &self.start.helpers
    }

    pub fn dirs(&self) -> &Dirs {
        &self.start.dirs
    }

    pub fn limits(&self) -> &Limits {
        &self.start.limits
    }

    pub fn begin(&mut self, item: &str) {
        self.reporter.begin(item);
    }

    pub fn item(&mut self, item: &str, value: Option<Value>) {
        self.reporter.item(item, value);
    }

    pub fn item_failed(&mut self, item: &str, error: JobError) {
        self.reporter.item_failed(item, error);
    }

    pub fn progress(&mut self, p: Progress) {
        self.reporter.progress(p);
    }

    /// Sends a checkpoint; `CHECKPOINT_TOO_LARGE` over 256 KiB of JSON.
    pub fn checkpoint(&mut self, data: Value) -> Result<(), JobError> {
        let msg = WorkerMsg::Checkpoint {
            job: self.start.job.id.clone(),
            data,
        };
        match self.reporter.shared.out.send(&msg) {
            Ok(()) => Ok(()),
            Err(CodecError::CheckpointTooLarge { bytes }) => Err(JobError::new(
                "CHECKPOINT_TOO_LARGE",
                format!("a checkpoint of {bytes} bytes passes 256 KiB"),
            )
            .with_details(serde_json::json!({ "bytes": bytes }))),
            Err(e) => Err(JobError::new("CHECKPOINT_TOO_LARGE", e.to_string())),
        }
    }

    pub fn cancelled(&self) -> Option<CancelReason> {
        self.token.reason()
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.token.clone()
    }

    pub fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    /// A host call; blocks this thread until the reply.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, JobError> {
        self.reporter.call(method, params)
    }

    pub fn diagnostics(&self) -> bool {
        self.start.diagnostics
    }

    /// Sends a `diag` when diagnostics are on; a no-op otherwise (Decision 7).
    pub fn diag(&mut self, kind: &'static str, payload: Value) {
        if self.start.diagnostics {
            send(
                &self.reporter.shared,
                &WorkerMsg::Diag {
                    job: Some(self.start.job.id.clone()),
                    kind: kind.to_owned(),
                    payload,
                },
            );
        }
    }

    /// The token bucket over `limits.readBytesPerSec`, shared by every clone; `None` without a
    /// limit.
    pub fn read_budget(&self) -> Option<ReadBudget> {
        self.budget.clone()
    }
}

/// Test-only hooks for the `test` family's protocol violations (feature `test-hooks`).
#[cfg(feature = "test-hooks")]
impl JobCtx<'_> {
    /// Stops `alive` for the rest of the process (`test.hang`).
    pub fn stop_alive_for_test(&self) {
        self.reporter.shared.pause_alive();
    }

    /// Stops reading stdin for the rest of the process: the host's writes fill the pipe and
    /// EOF no longer exits (`test.hang`: only the kill tree stops it).
    pub fn stop_reading_for_test(&self) {
        self.reporter.shared.stop_reading();
    }

    /// Sends any message with the next `seq` (`test.exitAfterResult`, `test.garbage`).
    pub fn send_for_test(&self, msg: &WorkerMsg) {
        send(&self.reporter.shared, msg);
    }

    /// Writes bytes as they are (`test.garbage`: bad JSON, a long line).
    pub fn send_raw_for_test(&self, bytes: &[u8]) {
        self.reporter.shared.out.send_raw(bytes);
    }

    /// Sends `{"v":1,"t": t,"seq":<next>}`: a type the host does not know.
    pub fn send_type_for_test(&self, t: &str) {
        self.reporter
            .shared
            .out
            .send_value(|seq| serde_json::json!({ "v": crate::PROTOCOL, "t": t, "seq": seq }));
    }

    /// Skips one `seq`, so the next message has a gap.
    pub fn skip_seq_for_test(&self) {
        self.reporter.shared.out.skip_seq();
    }

    /// Sends `{"t": t, "id": <fresh>}` and waits for its reply (`test.pluginMinor`).
    pub fn call_unknown_for_test(&self, t: &str) -> Result<Value, JobError> {
        let s = &self.reporter.shared;
        s.calls.call_raw(&s.out, t)
    }
}
